//! The WGSL emitter: what it writes, and whether `naga` will take it.
//!
//! Every claim here is either a parse and a validation at the `naga` version
//! Bevy pins, or a number in the uniform layout that the Bevy step will fill
//! bytes at. What is *not* claimed here is that the Bevy-flavoured module
//! compiles: it opens with `#import` lines and writes its bind group as
//! `#{MATERIAL_BIND_GROUP}`, both of which only `naga_oil` resolves, so what is
//! parsed is [`Shader::standalone`] — the same generated code with a literal
//! group, no imports and a stub entry point. The wrapper around it is checked
//! by the Bevy step compiling one.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]

use std::collections::BTreeSet;

use ashlar_material::{
    BlendMode, BrickOutput, Channel, Input, Material, MaterialGraph, MaterialGraphBuilder,
    MaterialGraphLibrary, MathOp, MirrorAxis, Param, PbrOutput, SdfOp, ShapeKind, ShapeOutput,
    SurfaceOutput, WeaveOutput, WeavePattern,
    ir::{Filter, IrType, Op, Target},
    nodes::{
        Adjust, Blend, Blur, Bricks, Buffer, CircleMap, CircleSplatter, Clamp, Colorize, Combine,
        Combine2, Curvature, Curve, CutFlag, Decompose, Dilate, Direction, DirectionalWarp,
        Distance, EdgeDetect, Erode, GradientWarp, GraphInput, HeightToMask, IntensityWarp, Invert,
        Kaleidoscope, Levels, Math, Mirror, Mix, Noise, NormalFromHeight, OcclusionFromHeight,
        Pattern, PatternKind, Scratches, SdfCombine, SdfMask, Shape, Subgraph, Switch, Tile, Tiles,
        Time, Transform, Triplanar, Uv, Voronoi, Warp, Weave, WorldMask, WorldNormal, WorldPos,
    },
    partition::{Partition, partition},
    stdlib,
    wgsl::{HASH, ParamLayout, Stage, UniformLayout, emit, emit_compute},
};

/// The resolution every partition here binds its textures at. Nothing in this
/// file rasterises a texel, so the smallest a bake takes is the right one.
const RESOLUTION: u32 = 256;

/// Parse and validate one module, or fail with the source and the diagnostic.
fn validate(source: &str, what: &str) {
    let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|error| {
        panic!(
            "{what} did not parse: {}\n{source}",
            error.emit_to_string(source)
        )
    });
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    if let Err(error) = validator.validate(&module) {
        panic!("{what} did not validate: {error:?}\n{source}");
    }
}

/// Emit every flavour of one partition and validate each.
///
/// The two fragment stages are validated through
/// [`Shader::standalone`](ashlar_material::wgsl::Shader::standalone); the
/// compute kernel imports nothing, so it is validated as it ships.
fn validate_every_flavour(split: &Partition, what: &str) {
    let fragment = emit(split, Stage::Fragment).unwrap();
    validate(&fragment.standalone(), &format!("{what}: the fragment"));
    let prepass = emit(split, Stage::Prepass).unwrap();
    validate(&prepass.standalone(), &format!("{what}: the prepass"));
    let compute = emit_compute(split, RESOLUTION).unwrap();
    validate(compute.module(), &format!("{what}: the compute kernel"));
    // The wrapper is text the Bevy step assembles, so the only claim made about
    // it here is that it is the wrapper.
    assert!(
        fragment.module().starts_with("#import bevy_pbr::"),
        "{what}"
    );
    assert!(
        fragment.module().contains("#{MATERIAL_BIND_GROUP}"),
        "{what}"
    );
    assert!(prepass.module().contains("prepass_alpha_discard"), "{what}");
}

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

/// A graph with one live `shift` parameter, built by the caller around it.
fn shifted(build: impl FnOnce(MaterialGraphBuilder) -> MaterialGraphBuilder) -> Partition {
    let material =
        build(MaterialGraph::builder("test:node").param(Param::float("shift", 0.25).live()))
            .build()
            .unwrap();
    partition(&material, &everything(&material), RESOLUTION).unwrap()
}

/// The ops the fragment evaluates, which is what the emitter printed.
fn fragment_ops(split: &Partition) -> Vec<Op> {
    let roots: Vec<_> = split
        .outputs()
        .values()
        .filter_map(|source| source.value())
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

// ---------------------------------------------------------------- every node

/// One graph per node kind, with the node's own instructions forced into the
/// fragment.
///
/// The recipe is the same for every kind: a [`Warp`] whose offset is a live
/// parameter, over the node under test. A warp is a resampler, so it re-emits
/// everything its source computes from the coordinate at a moved coordinate,
/// and a moved coordinate that depends on a uniform makes every one of those
/// copies runtime. That is what puts a generator's hash, a blend's arithmetic
/// and a shape's distance field into the fragment rather than into a texture.
///
/// The five buffered filters are the exception the partition documents: a blur
/// or a jump flood reads an unbounded neighbourhood and is a plane or nothing,
/// so what lands in the fragment for those is the [`Op::Sample`] that reads the
/// plane — which is the instruction the emitter has to get right for them.
///
/// One kind of the vocabulary is missing and is missing on purpose: a
/// `Comment` produces no signal, lowers to nothing, and cannot be wired to
/// anything, so there is no graph in which it emits.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one row per node kind; splitting it would hide that it is a table, \
              and that it has a row for every kind is the claim"
)]
fn node_graphs() -> Vec<(&'static str, Partition)> {
    let noise = || Noise::value().period(8);
    let mut graphs: Vec<(&'static str, Partition)> = vec![
        // The coordinate is a `Vec2`, and a warp reads a field, so it reaches
        // one through a lane of itself.
        (
            "Uv",
            shifted(|b| {
                b.node("under", Decompose::new(Uv::new(), Channel::R))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Noise",
            shifted(|b| {
                b.node("under", noise())
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Voronoi",
            shifted(|b| {
                b.node("under", Voronoi::new().period(4))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Bricks",
            shifted(|b| {
                b.node("under", Bricks::new().rows(4).columns(2))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Tiles",
            shifted(|b| {
                b.node("under", Tiles::new().rows(4).columns(4))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Pattern",
            shifted(|b| {
                b.node("under", Pattern::new(PatternKind::Checker).x(4).y(4))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Shape",
            shifted(|b| {
                b.node("under", Shape::new(ShapeKind::Circle))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            // A gear carries the arithmetic none of the other kinds does: the
            // angular fold, the tooth ramp and the radial reach, under a shell
            // and behind the distance output, so the emitter sees the whole of
            // what the third node set added to this node.
            "a hollow Gear as a distance",
            shifted(|b| {
                b.node(
                    "under",
                    Shape::new(ShapeKind::Gear)
                        .sides(9)
                        .size(0.42)
                        .depth(0.12)
                        .hollow(0.08)
                        .output(ShapeOutput::Distance),
                )
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            // And the two kinds whose distance is Euclidean, one rounded and
            // one a segment, read as masks.
            "a rounded Box beside a Capsule",
            shifted(|b| {
                b.node(
                    "panel",
                    Shape::new(ShapeKind::Box).size(0.4).edge(0.02).round(0.12),
                )
                .node(
                    "slot",
                    Shape::new(ShapeKind::Capsule)
                        .size(0.06)
                        .edge(0.01)
                        .length(0.3)
                        .hollow(0.03),
                )
                .node("under", Blend::new(BlendMode::Subtract, "panel", "slot"))
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Scratches",
            shifted(|b| {
                b.node("under", Scratches::new().count(8))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            // A twill as relief, which is the output carrying the whole of
            // what a cloth computes: both cross-sections, the crossing, and
            // the two crests the maximum chooses between.
            "a twill Weave as relief",
            shifted(|b| {
                b.node(
                    "under",
                    Weave::new()
                        .x(12)
                        .y(8)
                        .width(0.72)
                        .pattern(WeavePattern::Twill { step: 4 })
                        .output(WeaveOutput::Height),
                )
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            // And a satin read as per-thread ids, which is the arm that emits
            // the satin's own move, two hashes and the select between them.
            "a satin Weave as thread ids",
            shifted(|b| {
                b.node(
                    "under",
                    Weave::new()
                        .x(10)
                        .y(10)
                        .width(0.85)
                        .pattern(WeavePattern::Satin { step: 5 })
                        .seed(3)
                        .output(WeaveOutput::Id),
                )
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Transform",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Transform::new("src").scale(2.0))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Tile",
            shifted(|b| {
                b.node("src", Shape::new(ShapeKind::Circle))
                    .node("under", Tile::new("src").count(3))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Warp",
            shifted(|b| {
                b.node("src", noise())
                    .node("field", Noise::value().period(4).seed(3))
                    .node("under", Warp::new("src", "field"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "GradientWarp",
            shifted(|b| {
                b.node("src", noise())
                    .node("guide", Noise::perlin().period(4))
                    .node(
                        "n",
                        GradientWarp::new(
                            "src",
                            Math::new(MathOp::Mul, "guide", Input::param("shift")),
                        ),
                    )
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "IntensityWarp",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "n",
                        IntensityWarp::new("src", Input::param("shift")).angle(0.13),
                    )
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Mirror",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Mirror::new("src").axis(MirrorAxis::U))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Kaleidoscope",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Kaleidoscope::new("src"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // Two turns of the source round a disc, with a hole at the centre and a
        // twist, so the fragment gets the whole polar frame — the atan2, the
        // normalised radius and the select that fills outside the window —
        // rather than the degenerate case a default map would emit.
        (
            "CircleMap",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "under",
                        CircleMap::new("src")
                            .radius(0.4)
                            .inner(0.05)
                            .turns(2)
                            .twist(0.5)
                            .outside(0.25),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // Two rings of instances with every variation on and a mask that is a
        // field rather than a literal, so the fragment gets the whole ring
        // frame — the atan2, the rounding to a slot, the per-instance sine and
        // cosine and the gate — six times over rather than the one degenerate
        // instance a default splatter would emit.
        (
            "CircleSplatter",
            shifted(|b| {
                b.node("src", Shape::new(ShapeKind::Circle))
                    .node("gate", Noise::value().period(4).seed(5))
                    .node(
                        "under",
                        CircleSplatter::new("src")
                            .mask("gate")
                            .count(6)
                            .rings(2)
                            .radius(0.3)
                            .inner(0.08)
                            .scale(0.12)
                            .scale_variation(0.25)
                            .rotation_variation(0.4)
                            .radius_variation(0.3)
                            .opacity_variation(0.5)
                            .face_centre()
                            .seed(11),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Blend",
            shifted(|b| {
                b.node("a", noise())
                    .node("b", Noise::value().period(16).seed(1))
                    .node("under", Blend::new(BlendMode::Overlay, "a", "b"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Levels",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Levels::new("src").in_range(0.2, 0.8).gamma(1.4))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Curve",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "under",
                        Curve::new("src").points([[0.0, 0.1], [0.5, 0.8], [1.0, 0.3]]),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Colorize",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "under",
                        Colorize::new("src")
                            .gradient([(0.0, [0.1, 0.2, 0.3]), (1.0, [0.9, 0.8, 0.7])]),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            "Adjust",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "colour",
                        Colorize::new("src")
                            .gradient([(0.0, [0.1, 0.2, 0.3]), (1.0, [0.9, 0.8, 0.7])]),
                    )
                    .node("under", Adjust::new("colour").saturation(1.5).hue(30.0))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            "Math",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Math::new(MathOp::Pow, "src", 2.0))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Decompose",
            shifted(|b| {
                b.node("src", noise())
                    .node(
                        "colour",
                        Colorize::new("src")
                            .gradient([(0.0, [0.1, 0.2, 0.3]), (1.0, [0.9, 0.8, 0.7])]),
                    )
                    .node("under", Decompose::new("colour", Channel::B))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Combine",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Combine::new("src", 0.5, Noise::value().period(4)))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            // The two `Vec2` producers besides `Uv`. A warp reads a field and a
            // PBR channel takes one, and a `Vec2` converts to neither, so each
            // is read through a lane of itself — which still carries the node's
            // own compose into the fragment, because the lane moves with the
            // coordinate.
            "Combine2",
            shifted(|b| {
                b.node("src", noise())
                    .node("pair", Combine2::new("src", Noise::value().period(4)))
                    .node("under", Decompose::new("pair", Channel::G))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Direction",
            shifted(|b| {
                b.node("src", noise())
                    .node("comb", Direction::from_angle("src"))
                    .node("under", Decompose::new("comb", Channel::R))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            // The slope form is the buffered one: it reads the same plane a
            // `NormalFromHeight` reads, so what lands in the fragment for it is
            // the sample and the arithmetic that turns a normal back into a
            // gradient.
            "a slope Direction",
            shifted(|b| {
                b.node("src", noise())
                    .node("comb", Direction::from_slope("src").rotate_quarter())
                    .node("under", Decompose::new("comb", Channel::G))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Invert",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Invert::new("src"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Clamp",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Clamp::new("src").range(0.2, 0.7))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Mix",
            shifted(|b| {
                b.node("a", noise())
                    .node("b", Noise::value().period(4).seed(5))
                    .node("t", Noise::value().period(2).seed(6))
                    .node("under", Mix::new("a", "b", "t"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Switch",
            shifted(|b| {
                b.node("a", noise())
                    .node("b", Noise::value().period(4).seed(5))
                    .node("c", Noise::value().period(2).seed(6))
                    .node("under", Switch::new("c", "a", "b"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "NormalFromHeight",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", NormalFromHeight::new("src"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            "Blur",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Blur::new("src").radius(0.02))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "OcclusionFromHeight",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", OcclusionFromHeight::new("src").radius(0.03))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().occlusion("n"))
            }),
        ),
        (
            "Distance",
            shifted(|b| {
                b.node("src", noise())
                    .node("mask", Levels::new("src").in_low(0.7))
                    .node("under", Distance::new("mask"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "DirectionalWarp",
            shifted(|b| {
                b.node("src", noise())
                    .node("angle", Noise::value().period(4).seed(7))
                    .node("under", DirectionalWarp::new("src", "angle").amount(0.05))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "HeightToMask",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", HeightToMask::band("src", 0.3, 0.7).softness(0.1))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // A filleted subtraction, which is the branch of the boolean carrying
        // the most arithmetic: the negation, the clamped ramp, the mix and the
        // quadratic fillet, over two real distance fields rather than over
        // literals, so the emitter sees the whole polynomial.
        (
            "SdfCombine",
            shifted(|b| {
                b.node("plate", Shape::new(ShapeKind::Box).size(0.4).edge(0.0))
                    .node(
                        "hole",
                        Shape::new(ShapeKind::Circle)
                            .size(0.18)
                            .edge(0.0)
                            .output(ShapeOutput::Distance),
                    )
                    .node(
                        "under",
                        SdfCombine::new(SdfOp::Subtract, "plate", "hole").smooth(0.05),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "SdfMask",
            shifted(|b| {
                b.node(
                    "src",
                    Shape::new(ShapeKind::Star)
                        .sides(5)
                        .size(0.35)
                        .output(ShapeOutput::Distance),
                )
                .node("under", SdfMask::new("src").edge(0.04))
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Curvature",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Curvature::cavity("src").radius(0.02))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "EdgeDetect",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", EdgeDetect::new("src").radius(0.004))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "a directional Blur",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Blur::directional("src", 30.0).radius(0.02))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // Two planes behind one sample: the field and the height it is walked
        // down. Only the first is bound, because only the first is sampled —
        // which is the rule that a plane feeding a plane costs no binding.
        (
            "a slope Blur",
            shifted(|b| {
                b.node("src", noise())
                    .node("hill", Noise::value().period(4).seed(8))
                    .node("under", Blur::slope("src", "hill").radius(0.02).steps(4))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "a minimum slope Blur",
            shifted(|b| {
                b.node("src", noise())
                    .node("hill", Noise::value().period(4).seed(8))
                    .node(
                        "under",
                        Blur::slope("src", "hill")
                            .radius(0.02)
                            .steps(4)
                            .slope_mode(ashlar_material::nodes::SlopeMode::Min),
                    )
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Erode",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Erode::new("src").radius(0.01))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Dilate",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Dilate::new("src").radius(0.01))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "Buffer",
            shifted(|b| {
                b.node("src", noise())
                    .node("under", Buffer::new("src"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // The four runtime inputs need no warp: each is runtime by being what
        // it is, and each reaches an output directly.
        (
            "Time",
            shifted(|b| {
                b.node("n", Math::new(MathOp::Sin, Time::new(), 0.0))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "WorldPos",
            shifted(|b| {
                b.node("n", WorldPos::new())
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            "WorldNormal",
            shifted(|b| {
                b.node("n", WorldNormal::new())
                    .output(PbrOutput::new().base_color("n"))
            }),
        ),
        (
            "CutFlag",
            shifted(|b| {
                b.node("n", Math::new(MathOp::Mul, CutFlag::new(), 0.5))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        // And the two nodes built out of the world-space pair. The triplanar is
        // the one node in the vocabulary that emits its source three times, so
        // what is being validated here is three copies of a noise's hash at
        // three coordinates none of which is the texel's own.
        (
            "Triplanar",
            shifted(|b| {
                b.node("under", noise())
                    .node("n", Triplanar::new("under").tile_metres(2.5))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "WorldMask",
            shifted(|b| {
                b.node("up", WorldMask::up())
                    .node("low", WorldMask::below(2.0))
                    .node("n", Math::new(MathOp::Mul, "up", "low"))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
    ];
    graphs.push(("Subgraph", subgraph()));
    graphs.push(("GraphInput", bound_input()));
    graphs.extend(extra_graphs());
    graphs
}

/// Graphs for the ops no single node reaches on its own.
///
/// [`Math`] carries most of the arithmetic, and one node under test cannot
/// carry every operator it takes; and `Op::Normalize` is the partition's own,
/// emitted where a live height makes the material's normal four taps rather
/// than a bound map.
fn extra_graphs() -> Vec<(&'static str, Partition)> {
    vec![
        (
            "Math: the rest of the operators",
            shifted(|b| {
                b.node("src", Noise::value().period(8))
                    .node("root", Math::unary(MathOp::Sqrt, "src"))
                    .node("other", Noise::value().period(4).seed(9))
                    .node("angle", Math::new(MathOp::Atan2, "root", "other"))
                    .node("log", Math::unary(MathOp::Log2, "angle"))
                    .node("exp", Math::unary(MathOp::Exp2, "log"))
                    .node("under", Math::new(MathOp::Div, "exp", "other"))
                    .node("n", Warp::new("under", Input::param("shift")))
                    .output(PbrOutput::new().roughness("n"))
            }),
        ),
        (
            "a live height, whose normal is four taps",
            shifted(|b| {
                b.node("src", Noise::value().period(8))
                    .node("h", Math::new(MathOp::Mul, "src", Input::param("shift")))
                    .output(PbrOutput::new().height("h").normal_strength(0.02))
            }),
        ),
    ]
}

/// A graph that instances another, with the instance under a live warp.
fn subgraph() -> Partition {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:inner")
            .node("grain", Noise::value().period(8).seed(2))
            .output(PbrOutput::new().base_color("grain").roughness("grain"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("test:outer")
            .param(Param::float("shift", 0.25).live())
            .node(
                "under",
                Subgraph::new("test:inner").output(SurfaceOutput::Roughness),
            )
            .node("n", Warp::new("under", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
            .into_graph(),
    );
    let material = library.build("test:outer").unwrap();
    partition(&material, &everything(&material), RESOLUTION).unwrap()
}

/// The library the two cases below instance: one compound over a signal it does
/// not generate, exporting the signal beside the roughness it computes from it.
fn compounds() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:compound")
            .node("wear", GraphInput::float("wear", 0.25))
            .node("grain", Noise::value().period(8).seed(2))
            .node("worn", Blend::new(BlendMode::Multiply, "grain", "wear"))
            .output(PbrOutput::new().roughness("worn").extra("mask", "wear"))
            .into_graph(),
    );
    library
}

/// A graph that instances that compound with a live parameter wired into its
/// input, under a live warp.
///
/// This is the row for [`GraphInput`]: an input has a value only inside an
/// instance, so the only graph in which it emits anything is one that bound a
/// field to it. The field here is a uniform, which makes the whole instance
/// runtime — the compound's noise, its multiply and the read of the parameter
/// all land in the fragment, which is what the emitter has to get right.
fn bound_input() -> Partition {
    let mut library = compounds();
    library.insert(
        MaterialGraph::builder("test:outer")
            .param(Param::float("shift", 0.25).live())
            .node(
                "under",
                Subgraph::new("test:compound")
                    .input("wear", Input::param("shift"))
                    .output(SurfaceOutput::Roughness),
            )
            .node("n", Warp::new("under", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
            .into_graph(),
    );
    let material = library.build("test:outer").unwrap();
    partition(&material, &everything(&material), RESOLUTION).unwrap()
}

#[test]
fn every_node_emits_a_module_naga_takes() {
    for (kind, split) in node_graphs() {
        validate_every_flavour(&split, kind);
    }
}

#[test]
fn every_node_that_can_be_runtime_is_runtime() {
    // A buffered filter is a plane or it is nothing, so what a warp over one
    // puts in the fragment is the sample that reads the plane, and nothing of
    // the filter itself. Every other kind computes.
    let buffered = [
        "Blur",
        "OcclusionFromHeight",
        "Distance",
        "Erode",
        "Dilate",
        "Curvature",
        "EdgeDetect",
        "a directional Blur",
        "a slope Blur",
        "a slope Direction",
    ];
    for (kind, split) in node_graphs() {
        let ops = fragment_ops(&split);
        assert!(!ops.is_empty(), "{kind} put nothing in the fragment");
        if buffered.contains(&kind) {
            assert!(
                ops.iter().any(|op| matches!(op, Op::Sample(_))),
                "{kind} should reach its plane through a sample: {ops:?}"
            );
        }
    }
}

#[test]
fn two_nodes_reading_one_bound_instance_emit_it_once() {
    // A compound instanced twice on the same binding, once for the roughness
    // it computes and once for the mask it exports. The two graphs are the
    // same shape either way — one addition, over two values or over a value
    // and a constant — so a second copy of the instance would show as extra
    // hashes and a longer fragment, and there is neither.
    let split = |two_outputs: bool| {
        let mut library = compounds();
        let mut builder = MaterialGraph::builder("test:outer")
            .param(Param::float("shift", 0.25).live())
            .node(
                "a",
                Subgraph::new("test:compound")
                    .input("wear", Input::param("shift"))
                    .output(SurfaceOutput::Roughness),
            );
        builder = if two_outputs {
            builder
                .node(
                    "b",
                    Subgraph::new("test:compound")
                        .input("wear", Input::param("shift"))
                        .output(SurfaceOutput::Extra("mask".to_owned())),
                )
                .node("under", Math::new(MathOp::Add, "a", "b"))
        } else {
            builder.node("under", Math::new(MathOp::Add, "a", 0.5))
        };
        library.insert(
            builder
                .node("n", Warp::new("under", Input::param("shift")))
                .output(PbrOutput::new().roughness("n"))
                .into_graph(),
        );
        let material = library.build("test:outer").unwrap();
        partition(&material, &everything(&material), RESOLUTION).unwrap()
    };
    let one = fragment_ops(&split(false));
    let two = fragment_ops(&split(true));
    // The compound's noise is the part that costs something, and its hash is
    // the instruction a second copy would repeat.
    let hashes = |ops: &[Op]| ops.iter().filter(|op| matches!(op, Op::Hash2(_))).count();
    assert!(hashes(&one) > 0, "the instance is runtime: {one:?}");
    assert_eq!(hashes(&two), hashes(&one), "the instance was emitted twice");
    assert_eq!(two.len(), one.len(), "{two:?}");
}

#[test]
fn the_node_graphs_between_them_emit_every_op_the_interpreter_runs() {
    // The emitter is the second implementation of the op set the conformance
    // test compares, so a node table that never reached an arm would leave that
    // arm unwritten and unnoticed.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (_, split) in node_graphs() {
        for op in fragment_ops(&split) {
            seen.insert(tag(op));
        }
    }
    let wanted = [
        Op::Const(0.0),
        Op::Uv,
        Op::Param(0),
        Op::Time,
        Op::WorldPos,
        Op::WorldNormal,
        Op::CutFlag,
        Op::Add,
        Op::Sub,
        Op::Mul,
        Op::Div,
        Op::Min,
        Op::Max,
        Op::Abs,
        Op::Floor,
        Op::Fract,
        Op::Sqrt,
        Op::Pow,
        Op::Exp2,
        Op::Log2,
        Op::Sin,
        Op::Cos,
        Op::Atan2,
        Op::Mix,
        Op::Step,
        Op::Smoothstep,
        Op::Clamp,
        Op::Select,
        Op::Hash2(0),
        Op::Length,
        Op::Dot,
        Op::Normalize,
        Op::Sample(ashlar_material::ir::BufferId::default()),
        Op::Compose,
        Op::Extract(0),
    ];
    let missing: Vec<String> = wanted
        .iter()
        .map(|op| tag(*op))
        .filter(|name| !seen.contains(name))
        .collect();
    assert!(missing.is_empty(), "never emitted: {missing:?}");
}

/// An op's kind, so two ops of a kind with different payloads count as one.
///
/// Read off the debug name rather than off the crate's own `Op::code`, which is
/// private: a test outside the crate has the derived `Debug` and nothing else,
/// and the name before the payload is the kind.
fn tag(op: Op) -> String {
    format!("{op:?}")
        .split('(')
        .next()
        .unwrap_or_default()
        .to_owned()
}

// ---------------------------------------------- the standard library

/// Every graph this crate ships to a game: [`stdlib`], built in Rust.
///
/// The loops below run over all of it. A compound is shipped to be
/// *instanced*, by a caller who will partition it with their own parameters
/// live and never open its WGSL, so "it emits a module `naga` takes" is the
/// only thing standing between a compound and a driver, and it is worth
/// asserting over the whole library rather than over the one graph somebody
/// remembered.
fn shipped() -> MaterialGraphLibrary {
    stdlib::graphs()
}

#[test]
fn every_shipped_graph_with_every_parameter_live_emits_a_module_naga_takes() {
    let library = shipped();
    for (key, material) in library.build_all().unwrap() {
        let split = match partition(&material, &everything(&material), 512) {
            Ok(split) => split,
            Err(error) if key == "library:grass" => {
                assert!(error.reason.contains("binds 11 textures"), "{error}");
                // The preserved grass reference is shipped baked. Its all-live
                // variant exceeds the fragment budget; its baked variant must pass.
                partition(&material, &Target::Shader { live: Vec::new() }, 512).unwrap()
            }
            Err(error) => panic!("{key}: {error}"),
        };
        validate_every_flavour(&split, &key);
        let shader = emit(&split, Stage::Fragment).unwrap();
        // Every image the partition bound has a binding pair, and the pairs run
        // from 101 without a gap.
        assert_eq!(shader.bindings().len(), split.images(), "{key}");
        for (index, binding) in shader.bindings().iter().enumerate() {
            let index = u32::try_from(index).unwrap();
            assert_eq!(binding.texture, 101 + 2 * index, "{key}");
            assert_eq!(binding.sampler, 102 + 2 * index, "{key}");
        }
        // The block is the live set, in order, and it is a whole number of
        // sixteen-byte rows.
        assert_eq!(shader.layout().entries().len(), split.live().len(), "{key}");
        assert_eq!(shader.layout().size() % 16, 0, "{key}");
    }
}

#[test]
fn every_shipped_graph_with_nothing_live_is_a_module_of_texture_reads() {
    let library = shipped();
    for (key, material) in library.build_all().unwrap() {
        let split = partition(&material, &Target::Shader { live: Vec::new() }, 512).unwrap();
        validate_every_flavour(&split, &key);
        let shader = emit(&split, Stage::Fragment).unwrap();
        // No shipped graph reads a runtime input or declares a parameter live:
        // a compound reads none by construction — that is the rule that lets
        // it be baked and tested alone — and a library surface is a bake. So
        // with nothing live each one folds to textures and constants.
        assert!(
            split.is_static(),
            "{key} keeps something runtime with nothing live"
        );
        // For the rest: the block is the empty sixteen bytes every graph
        // carries and the fragment is samples and nothing else.
        assert!(shader.layout().entries().is_empty(), "{key}");
        assert_eq!(shader.layout().size(), 16, "{key}");
        assert!(shader.core().contains("_empty: vec4<f32>"), "{key}");
        for op in fragment_ops(&split) {
            assert!(
                matches!(op, Op::Sample(_) | Op::Uv | Op::Const(_) | Op::Compose),
                "{key}: {op:?} in a fragment that should only read textures"
            );
        }
    }
}

// ------------------------------------------------------------- the layout

#[test]
fn the_uniform_block_lays_out_the_way_std140_says() {
    // Two floats, a colour, two more floats, a colour: the declaration order,
    // with the two conversions a graph makes — an integer and a switch are
    // floats — and nothing reordered.
    let material = MaterialGraph::builder("test:block")
        .param(Param::float("wear", 0.5).live())
        .param(Param::int("count", 3).live())
        .param(Param::color("tint", [0.2, 0.3, 0.4]).live())
        .param(Param::bool("lit", true).live())
        .param(Param::float("gloss", 0.25).live())
        .param(Param::color("grime", [0.1, 0.1, 0.1]).live())
        .node("src", Noise::value().period(8))
        .node("a", Math::new(MathOp::Mul, "src", Input::param("wear")))
        .node("b", Math::new(MathOp::Add, "a", Input::param("count")))
        .node("c", Math::new(MathOp::Add, "b", Input::param("lit")))
        .node("d", Math::new(MathOp::Add, "c", Input::param("gloss")))
        .node(
            "tinted",
            Blend::new(BlendMode::Multiply, "d", Input::param("tint")),
        )
        .node(
            "dirty",
            Blend::new(BlendMode::Multiply, "tinted", Input::param("grime")),
        )
        .output(PbrOutput::new().base_color("dirty").roughness("d"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let shader = emit(&split, Stage::Fragment).unwrap();
    let layout = shader.layout();
    let seen: Vec<(&str, u32, u32)> = layout
        .entries()
        .iter()
        .map(|entry| (entry.name.as_str(), entry.offset, entry.size))
        .collect();
    // Hand-computed: a float aligns to four and a colour to sixteen, and a
    // colour is twelve bytes in a sixteen-byte slot, so the float after one
    // starts in that slot's fourth lane.
    assert_eq!(
        seen,
        vec![
            ("wear", 0, 4),
            ("count", 4, 4),
            // 8 is not a multiple of sixteen, so the colour moves to 16.
            ("tint", 16, 12),
            ("lit", 28, 4),
            ("gloss", 32, 4),
            // 36 is not a multiple of sixteen either.
            ("grime", 48, 12),
        ]
    );
    // 60 bytes used, rounded up to the next row.
    assert_eq!(layout.size(), 64);
    // And the shader agrees, because the tail is a member with an explicit size
    // rather than a member of its own.
    assert!(shader.core().contains("@size(4) _tail: f32"));
    validate(&shader.standalone(), "the mixed block");

    // The bytes a caller writes land where the layout says they do.
    let values = [
        [0.5, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [0.2, 0.3, 0.4],
        [1.0, 0.0, 0.0],
        [0.25, 0.0, 0.0],
        [0.1, 0.1, 0.1],
    ];
    let bytes = layout.bytes(&values);
    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[0..4], &0.5_f32.to_le_bytes());
    assert_eq!(&bytes[4..8], &3.0_f32.to_le_bytes());
    // The eight bytes of padding before the colour belong to nobody.
    assert_eq!(&bytes[8..16], &[0_u8; 8]);
    assert_eq!(&bytes[16..20], &0.2_f32.to_le_bytes());
    assert_eq!(&bytes[24..28], &0.4_f32.to_le_bytes());
    assert_eq!(&bytes[28..32], &1.0_f32.to_le_bytes());
    assert_eq!(&bytes[48..52], &0.1_f32.to_le_bytes());
    assert_eq!(&bytes[60..64], &[0_u8; 4]);
}

#[test]
fn a_block_with_no_parameters_is_still_a_block() {
    let material = MaterialGraph::builder("test:cold")
        .node("src", Noise::value().period(8))
        .node("n", Math::new(MathOp::Mul, "src", Time::new()))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    let split = partition(&material, &Target::Shader { live: Vec::new() }, RESOLUTION).unwrap();
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(shader.layout().entries().is_empty());
    // Sixteen bytes and one padding member, so the bind group layout does not
    // change shape from one graph to the next.
    assert_eq!(shader.layout().size(), 16);
    assert_eq!(shader.layout().bytes(&[]).len(), 16);
    validate(&shader.standalone(), "an empty block");
}

#[test]
fn a_parameter_whose_name_is_not_a_wgsl_identifier_is_renamed_and_still_found() {
    let material = MaterialGraph::builder("test:names")
        // A reserved word, a name with punctuation, and a name that collides
        // with the first one once the punctuation is replaced.
        .param(Param::float("filter", 0.5).live())
        .param(Param::float("wear-amount", 0.5).live())
        .param(Param::float("wear amount", 0.5).live())
        .node("src", Noise::value().period(8))
        .node("a", Math::new(MathOp::Mul, "src", Input::param("filter")))
        .node(
            "b",
            Math::new(MathOp::Add, "a", Input::param("wear-amount")),
        )
        .node(
            "c",
            Math::new(MathOp::Add, "b", Input::param("wear amount")),
        )
        .output(PbrOutput::new().roughness("c"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let shader = emit(&split, Stage::Fragment).unwrap();
    let members: Vec<&str> = shader
        .layout()
        .entries()
        .iter()
        .map(|entry| entry.member.as_str())
        .collect();
    assert_eq!(members, vec!["filter_0", "wear_amount", "wear_amount_2"]);
    // The graph's own names are what a caller looks a parameter up by.
    assert_eq!(
        shader.layout().entry("wear amount").map(|e| e.offset),
        Some(8)
    );
    validate(&shader.standalone(), "a block of awkward names");
}

// ------------------------------------------------------------- the hash

#[test]
fn the_lattice_hash_is_the_documented_sequence() {
    // A GPU is not available here and `naga` is a front end rather than an
    // evaluator, so what is asserted is the arithmetic itself: the primes, the
    // shifts, the multiply, the wrapping reduction and the divisor. Numeric
    // equality with the interpreter is the conformance test's, which dispatches
    // the compute kernel this text ends up in.
    for line in [
        "const ASHLAR_PRIME_0: u32 = 374761393u;",
        "const ASHLAR_PRIME_1: u32 = 668265263u;",
        "const ASHLAR_PRIME_2: u32 = 3266489917u;",
        "const ASHLAR_PRIME_3: u32 = 2654435761u;",
        "const ASHLAR_AVALANCHE: u32 = 1274126177u;",
        // `u32::MAX as f32` rounds up, and the shader divides by what the
        // interpreter divided by rather than by the exact integer.
        "const ASHLAR_HASH_SCALE: f32 = 4294967296.0;",
        "let shifted = (mixed ^ (mixed >> 13u)) * ASHLAR_AVALANCHE;",
        "return shifted ^ (shifted >> 16u);",
        "x * ASHLAR_PRIME_0 + y * ASHLAR_PRIME_1 + seed * ASHLAR_PRIME_3",
        "return f32(bits) / ASHLAR_HASH_SCALE;",
        // The lattice reduction: floor, then a Euclidean remainder, and no
        // reduction at all below a period of one.
        "let floored = floor(coordinate);",
        "if period >= 1.0 {",
        "var remainder = floored % period;",
        "remainder = remainder + abs(period);",
        "return bitcast<u32>(i32(reduced));",
    ] {
        assert!(HASH.contains(line), "the hash does not say {line:?}");
    }

    // And a graph with a noise in the fragment carries exactly that text.
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .node("n", Warp::new("src", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
    });
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(shader.core().contains(HASH));
    assert!(shader.core().contains("ashlar_hash2("));
}

#[test]
fn a_graph_with_no_hash_in_its_fragment_carries_no_hash_function() {
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .node("n", Math::new(MathOp::Mul, "src", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
    });
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(!shader.core().contains("ashlar_hash2"));
    validate(&shader.standalone(), "a fragment with no hash");
}

// -------------------------------------------------------------- the clock

#[test]
fn the_prepass_reads_the_clock_from_the_group_the_prepass_binds_it_in() {
    // One buffer, two bind groups. The main pass's view group is
    // `mesh_view_bindings`, where `globals` is binding 11 and is imported by
    // name. The prepass has a view group of its own, where the same buffer is
    // bound at binding 1 and no shipped module declares it — Bevy binds it and
    // none of its own prepass shaders read it. Importing the main pass's name
    // into a prepass is a pipeline wgpu refuses with "binding 11 is not
    // available in the pipeline layout", which is how this was found: the
    // preview rendering the study's light strip with a normal prepass on.
    let material = MaterialGraph::builder("test:clock")
        .node("grain", Noise::value().period(8))
        .node("now", Time::new())
        .node("wave", Math::new(MathOp::Sin, "now", 0.0))
        .node("lit", Math::new(MathOp::Mul, "grain", "wave"))
        .output(
            PbrOutput::new()
                .emissive("lit")
                .height("grain")
                .normal_strength(0.01),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();

    let fragment = emit(&split, Stage::Fragment).unwrap().module().to_owned();
    assert!(fragment.contains("#import bevy_pbr::mesh_view_bindings::globals"));
    assert!(fragment.contains("globals.time"));
    assert!(!fragment.contains("@group(0) @binding(1)"));

    let prepass = emit(&split, Stage::Prepass).unwrap().module().to_owned();
    assert!(
        prepass.contains("#import bevy_render::globals::Globals"),
        "{prepass}"
    );
    assert!(
        prepass.contains("@group(0) @binding(1) var<uniform> ashlar_globals: Globals;"),
        "{prepass}"
    );
    // At the start of its own line, like every other generated declaration:
    // this is the line an author reads when a clock binding goes wrong again.
    assert!(
        prepass
            .lines()
            .any(|line| line == "@group(0) @binding(1) var<uniform> ashlar_globals: Globals;"),
        "{prepass}"
    );
    assert!(prepass.contains("ashlar_globals.time"), "{prepass}");
    assert!(
        !prepass.contains("mesh_view_bindings::globals"),
        "{prepass}"
    );
}

#[test]
fn a_graph_with_no_clock_declares_none() {
    // The declaration is not free — it claims a binding of the prepass view
    // group — so a graph that never reads the clock must not make it.
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .output(PbrOutput::new().roughness("src"))
    });
    for stage in [Stage::Fragment, Stage::Prepass] {
        let module = emit(&split, stage).unwrap().module().to_owned();
        assert!(!module.contains("globals"), "{stage:?}: {module}");
        assert!(!module.contains("Globals"), "{stage:?}: {module}");
    }
}

// ------------------------------------------------------------- the normal

#[test]
fn a_bound_normal_map_is_decoded_and_a_derived_one_is_not() {
    // A static height gives a bound normal map, which the bake writes into an
    // eight-bit unorm image half and half about zero, so the shader undoes
    // exactly that.
    let material = MaterialGraph::builder("test:relief")
        .param(Param::float("tint", 0.5).live())
        .node("src", Noise::value().period(8))
        .node("lit", Math::new(MathOp::Mul, "src", Input::param("tint")))
        .output(
            PbrOutput::new()
                .base_color("lit")
                .height("src")
                .normal_strength(0.02),
        )
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let normal = split
        .textures()
        .iter()
        .find(|texture| texture.port.as_deref() == Some("normal"))
        .expect("a static height binds a normal map");
    assert!(matches!(normal.filter, Filter::Normal { .. }));
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(
        shader.core().contains(".rgb * 2.0 - 1.0)"),
        "a unorm normal map is decoded: {}",
        shader.core()
    );
    validate(&shader.standalone(), "a bound normal map");

    // A live height has no plane, so the normal is the four taps the partition
    // emitted, and there is nothing to decode.
    let live = MaterialGraph::builder("test:live-relief")
        .param(Param::float("relief", 0.5).live())
        .node("src", Noise::value().period(8))
        .node("h", Math::new(MathOp::Mul, "src", Input::param("relief")))
        .output(PbrOutput::new().height("h").normal_strength(0.02))
        .build()
        .unwrap();
    let split = partition(&live, &everything(&live), RESOLUTION).unwrap();
    assert!(split.report().taps >= 4, "{}", split.report());
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(!shader.core().contains("* 2.0 - 1.0)"));
    assert!(shader.core().contains("ashlar_normalize_vec3"));
    validate(&shader.standalone(), "a derived normal");
}

#[test]
fn the_fragment_writes_the_slots_it_bound_and_no_others() {
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .node("n", Math::new(MathOp::Mul, "src", Input::param("shift")))
            .output(
                PbrOutput::new()
                    .roughness("n")
                    .height("src")
                    .normal_strength(0.02),
            )
    });
    let shader = emit(&split, Stage::Fragment).unwrap();
    // The surface function answers every port the graph bound; the entry point
    // is what decides which of them reaches a slot, so that is what is read.
    let entry = shader
        .module()
        .split("@fragment")
        .nth(1)
        .expect("the module carries an entry point");
    assert!(entry.contains("pbr_input.material.perceptual_roughness"));
    assert!(entry.contains("pbr_input.material.base_color"));
    assert!(entry.contains("pbr_input.diffuse_occlusion"));
    // A height is baked and addressable and bound to no `StandardMaterial`
    // slot, exactly as a baked surface's height map is.
    assert!(shader.core().contains("surface.height"));
    assert!(!entry.contains("surface.height"));
    // The graph binds no emissive, so nothing writes one.
    assert!(!entry.contains("surface.emissive"));
    // The graph binds a height and a strength, so a normal is derived.
    assert!(entry.contains("calculate_tbn_mikktspace"));
}

// ------------------------------------------------------------- the compute

#[test]
fn the_compute_kernel_covers_the_square_and_names_its_ports() {
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .node("n", Math::new(MathOp::Mul, "src", Input::param("shift")))
            .output(PbrOutput::new().roughness("n").emissive("src"))
    });
    let compute = emit_compute(&split, RESOLUTION).unwrap();
    validate(compute.module(), "the compute kernel");
    assert_eq!(compute.resolution(), RESOLUTION);
    let ports: Vec<&str> = compute
        .ports()
        .iter()
        .map(|(port, _)| port.as_str())
        .collect();
    assert_eq!(
        ports,
        vec![
            "base_color",
            "emissive",
            "metallic",
            "occlusion",
            "roughness"
        ]
    );
    assert_eq!(compute.output_len(), 256 * 256 * 5);
    // A colour port fills three lanes and a scalar one.
    assert_eq!(
        compute
            .ports()
            .iter()
            .filter(|(_, t)| *t == IrType::Vec3)
            .count(),
        2
    );
    // The texel centre, which is where the CPU's own plane is evaluated.
    assert!(compute.module().contains(
        "(vec2<f32>(f32(id.x), f32(id.y)) + vec2<f32>(0.5, 0.5)) / f32(ASHLAR_RESOLUTION)"
    ));
    // No derivatives in a compute stage, so the reads are explicit-level.
    assert!(compute.module().contains("textureSampleLevel("));
    assert!(!compute.module().contains("textureSample("));
    assert!(
        compute
            .module()
            .contains("@compute @workgroup_size(8, 8, 1)")
    );
    // It imports nothing, so what is validated is what ships.
    assert!(!compute.module().contains("#import"));
    assert!(!compute.module().contains("#{"));
}

/// Every stage a GPU bake dispatches is a module `naga` takes, for every graph
/// this repository ships.
///
/// The fragment's own coverage, one step over: a plane kernel is written for a
/// port the PBR output never names and over a bind group that holds only the
/// planes already made, and neither of those shapes is reached by emitting a
/// whole partition. A GPU is what finds a driver's opinion; this is what finds
/// a module that is not WGSL.
#[test]
fn every_stage_of_a_compute_bake_emits_a_module_naga_takes() {
    use ashlar_material::{
        ir::lower,
        partition::{over_planes, plane_port},
        wgsl::emit_compute_ports,
    };

    let library = shipped();
    let mut stages = 0;
    for (key, material) in library.build_all().unwrap() {
        // A GPU bake is a bake, so a graph a bake refuses has no stages at all:
        // `study:concrete-wet` reads the fragment's own world normal and
        // height and `study:brick-weathered` reads its world normal, and
        // neither backend has one. The refusal is `tests/partition.rs`'s to
        // check; here it is simply not a case.
        let Ok(ir) = lower(&material, Target::Bake) else {
            continue;
        };
        let split = match over_planes(&ir, RESOLUTION) {
            Ok(split) => split,
            Err(error)
                if matches!(
                    key.as_str(),
                    "library:grass"
                        | "library:moss-carpet"
                        | "library:soi-cobblestone-moss-heavy"
                        | "library:soi-cobblestone-moss-light"
                ) =>
            {
                assert!(
                    error
                        .reason
                        .contains("textures, and a compiled material may bind 8"),
                    "{key}: {error}"
                );
                // These preserved complex references use CPU content bakes.
                continue;
            }
            Err(error) => panic!("{key}: {error}"),
        };
        for (index, plan) in ir.buffers().iter().enumerate() {
            let size = plan.resolution.unwrap_or(RESOLUTION);
            let kernel = emit_compute_ports(&split, size, &[plane_port(index)]).unwrap();
            validate(kernel.module(), &format!("{key} plane {index}"));
            assert_eq!(kernel.ports().len(), 1);
            stages += 1;
        }
        let outputs: Vec<String> = ir.roots().keys().cloned().collect();
        let kernel = emit_compute_ports(&split, RESOLUTION, &outputs).unwrap();
        validate(kernel.module(), &format!("{key} outputs"));
        assert_eq!(kernel.ports().len(), outputs.len());
        stages += 1;
    }
    // Every stage of every shipped graph: one outputs stage per graph, plus one
    // per buffered plane. A drop here is coverage quietly going away, and a
    // rise is a graph that grew a plane; either way, read the diff and move
    // the number deliberately.
    assert_eq!(stages, 164, "the shipped library's stages");
}

#[test]
fn a_kernel_asked_for_mesh_planes_reads_a_position_normal_and_cut_flag_per_texel() {
    use ashlar_material::wgsl::{MeshInputs, emit_compute_mesh};

    let split = shifted(|b| {
        b.node("grain", Noise::value().period(8))
            .node("under", Triplanar::new("grain").tile_metres(2.0))
            .node("sawn", Math::new(MathOp::Mul, "under", CutFlag::new()))
            .node("n", Math::new(MathOp::Mul, "sawn", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
    });
    let ports: Vec<String> = split.outputs().keys().cloned().collect();

    // The default is what a GPU bake dispatches: one position, one normal and
    // one flag for the whole square, out of the uniform block a dispatch is
    // told.
    let fixed = emit_compute_mesh(&split, RESOLUTION, &ports, MeshInputs::Uniform, 0).unwrap();
    validate(fixed.module(), "a kernel over a fixed mesh");
    assert_eq!(fixed.mesh(), MeshInputs::Uniform);
    assert!(fixed.module().contains("ashlar_inputs.world_pos"));
    assert!(fixed.module().contains("ashlar_inputs.cut_flag"));
    assert!(!fixed.module().contains("ashlar_world_pos"));
    assert!(!fixed.module().contains("ashlar_cut_flag"));

    // And the shape the conformance harness feeds a graph that reads the mesh:
    // three storage buffers, one texel each, read at the same row-major index
    // the answers are written at.
    let planes = emit_compute_mesh(&split, RESOLUTION, &ports, MeshInputs::Planes, 0).unwrap();
    validate(planes.module(), "a kernel over mesh planes");
    assert_eq!(planes.mesh(), MeshInputs::Planes);
    assert_eq!(planes.mesh_len(), (RESOLUTION as usize).pow(2));
    for declaration in [
        "@group(0) @binding(97) var<storage, read> ashlar_world_pos: array<vec4<f32>>;",
        "@group(0) @binding(98) var<storage, read> ashlar_world_normal: array<vec4<f32>>;",
        "@group(0) @binding(96) var<storage, read> ashlar_cut_flag: array<vec4<f32>>;",
    ] {
        assert!(planes.module().contains(declaration), "{declaration}");
    }
    assert!(
        planes.module().contains(
            "ashlar_world_pos[texel].xyz, ashlar_world_normal[texel].xyz, \
             ashlar_cut_flag[texel].x"
        ),
        "{}",
        planes.module()
    );
    // The clock still comes from the uniform: only the three things a mesh has
    // are laid over the square.
    assert!(planes.module().contains("ashlar_inputs.time"));
    assert!(!planes.module().contains("ashlar_inputs.cut_flag"));
    // Nothing else about the kernel moves. The surface function takes the same
    // arguments either way, which is what keeps one emitter answering for the
    // fragment, the prepass and both flavours of dispatch.
    assert_eq!(planes.ports(), fixed.ports());
    assert_eq!(planes.output_len(), fixed.output_len());
    assert_eq!(planes.bindings(), fixed.bindings());
}

/// The two stages a mesh attribute reaches, and the guard around the fetch.
///
/// `ashlar-bevy` puts the flag in the mesh's second UV set, which Bevy's vertex
/// stages forward as `uv_b` under `VERTEX_UVS_B` and which a mesh with no cut
/// face does not carry at all. So the fetch is guarded and its `#else` is zero,
/// and a graph that never asks for the flag emits neither the guard nor the
/// dependence on a vertex attribute.
#[test]
fn a_fragment_reading_the_cut_flag_takes_it_from_the_second_uv_set() {
    let sawn = shifted(|b| {
        b.node("grain", Noise::value().period(8))
            .node(
                "raw",
                Math::new(MathOp::Mul, "grain", Input::param("shift")),
            )
            .node("n", Mix::new("grain", "raw", CutFlag::new()))
            // A runtime height, so the prepass has a normal to write and its
            // entry point is emitted at all.
            .output(
                PbrOutput::new()
                    .roughness("n")
                    .height("n")
                    .normal_strength(0.02),
            )
    });
    for stage in [Stage::Fragment, Stage::Prepass] {
        let module = emit(&sawn, stage).unwrap().module().to_owned();
        assert!(
            module.contains(
                "#ifdef VERTEX_UVS_B\n    let ashlar_cut = in.uv_b.x;\n#else\n    let \
                 ashlar_cut = 0.0;\n#endif\n"
            ),
            "{stage:?}: {module}"
        );
        assert!(
            module.contains("ashlar_cut);"),
            "{stage:?}: the flag reaches the surface function\n{module}"
        );
    }

    // A graph that does not read it is exactly what it was: the surface
    // function still takes the argument, and the entry point passes a zero.
    let plain = shifted(|b| {
        b.node("grain", Noise::value().period(8))
            .node("n", Math::new(MathOp::Mul, "grain", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
    });
    for stage in [Stage::Fragment, Stage::Prepass] {
        let module = emit(&plain, stage).unwrap().module().to_owned();
        assert!(!module.contains("VERTEX_UVS_B"), "{stage:?}: {module}");
        assert!(!module.contains("uv_b"), "{stage:?}: {module}");
    }
}

#[test]
fn a_resolution_a_bake_would_not_take_is_refused_by_path() {
    let split = shifted(|b| {
        b.node("src", Noise::value().period(8))
            .node("n", Math::new(MathOp::Mul, "src", Input::param("shift")))
            .output(PbrOutput::new().roughness("n"))
    });
    let error = emit_compute(&split, 300).unwrap_err();
    assert_eq!(error.path, "resolution");
}

// ------------------------------------------------------------- the literals

#[test]
fn a_folded_constant_reaches_the_shader_exactly() {
    // `log2(0)` folds to an infinity, which has no WGSL literal at all, so it
    // is written as the bits it is rather than as a number that is nearly it.
    let material = MaterialGraph::builder("test:literals")
        .param(Param::float("shift", 0.25).live())
        .node("zero", Math::unary(MathOp::Log2, 0.0))
        .node("odd", Math::new(MathOp::Add, "zero", 0.1))
        .node("src", Noise::value().period(8))
        .node("n", Math::new(MathOp::Mul, "src", Input::param("shift")))
        .node("out", Math::new(MathOp::Min, "n", "odd"))
        .output(PbrOutput::new().roughness("out"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let shader = emit(&split, Stage::Fragment).unwrap();
    assert!(
        shader.core().contains("bitcast<f32>(0xff800000u)"),
        "{}",
        shader.core()
    );
    validate(&shader.standalone(), "a module with an infinity in it");
}

#[test]
fn the_layout_type_is_public_enough_to_fill_a_block_from() {
    // What the Bevy step reads: a name, an offset, a size, and a width.
    let material = MaterialGraph::builder("test:one")
        .param(Param::color("tint", [0.1, 0.2, 0.3]).live())
        .node("src", Noise::value().period(8))
        .node(
            "n",
            Blend::new(BlendMode::Multiply, "src", Input::param("tint")),
        )
        .output(PbrOutput::new().base_color("n"))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    let shader = emit(&split, Stage::Fragment).unwrap();
    let layout: &UniformLayout = shader.layout();
    let entry: &ParamLayout = layout.entry("tint").unwrap();
    assert_eq!(entry.offset, 0);
    assert_eq!(entry.size, 12);
    assert_eq!(entry.value_type, IrType::Vec3);
    assert_eq!(layout.size(), 16);
}

#[test]
fn a_brick_output_that_is_a_colour_still_writes_a_colour_slot() {
    let split = shifted(|b| {
        b.node(
            "under",
            Bricks::new().rows(4).columns(2).output(BrickOutput::Id),
        )
        .node("n", Warp::new("under", Input::param("shift")))
        .output(PbrOutput::new().base_color("n"))
    });
    let shader = emit(&split, Stage::Fragment).unwrap();
    let ports: Vec<&str> = shader.ports().iter().map(|(p, _)| p.as_str()).collect();
    assert!(ports.contains(&"base_color"));
    validate(&shader.standalone(), "a brick id as a colour");
}

#[test]
fn a_packed_image_is_read_by_the_lane_the_binding_owns() {
    // Ten separate static scalars, each behind its own live multiply, so each is
    // a frontier of its own and there are more of them than a bind group holds.
    // The partition then packs them four to an RGBA image, and the shader has to
    // read the lane each one landed in rather than always the red channel.
    let mut builder = MaterialGraph::builder("test:packed").param(Param::float("wear", 0.5).live());
    let mut total: Option<String> = None;
    for index in 0..10_u32 {
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
        .output(PbrOutput::new().roughness(Input::node(total.unwrap())))
        .build()
        .unwrap();
    let split = partition(&material, &everything(&material), RESOLUTION).unwrap();
    assert_eq!(split.report().bindings, 10);
    assert_eq!(split.report().textures, 3, "{}", split.report());
    let shader = emit(&split, Stage::Fragment).unwrap();
    // Three images, three binding pairs, and every lane of the first two read.
    assert_eq!(shader.bindings().len(), 3);
    let core = shader.core();
    let lanes: BTreeSet<char> = core
        .match_indices("ashlar_sampler_0, ")
        .filter_map(|(at, _)| {
            let rest = core.get(at..)?;
            let close = rest.find(").")?;
            rest.as_bytes().get(close + 2).map(|byte| char::from(*byte))
        })
        .collect();
    assert_eq!(
        lanes,
        "rgba".chars().collect::<BTreeSet<char>>(),
        "every lane of the first packed image should be read:\n{core}"
    );
    validate(&shader.standalone(), "a packed image");
}

#[test]
fn a_tangent_space_normal_is_read_under_the_def_that_declares_a_tangent() {
    // `VertexOutput` carries `world_tangent` only under `VERTEX_TANGENTS`, in
    // both the main pass and the prepass, and Bevy's own `pbr_fragment` guards
    // every use of it. A mesh with no tangent attribute would otherwise not
    // compile a pipeline at all, where what it should get is the flat surface a
    // mesh with no normal map gets. Every mesh this workspace uploads generates
    // tangents; `ProceduralMaterial` is public, and a caller's may not.
    let material = MaterialGraph::builder("test:tangents")
        .node("grain", Noise::value().period(8))
        .output(
            PbrOutput::new()
                .base_color("grain")
                .height("grain")
                .normal_strength(0.4),
        )
        .build()
        .unwrap();
    let split = partition(&material, &Target::Shader { live: Vec::new() }, RESOLUTION).unwrap();
    for stage in [Stage::Fragment, Stage::Prepass] {
        let text = emit(&split, stage).unwrap().module().to_owned();
        assert!(
            text.contains("in.world_tangent"),
            "a normal is bound, so the tangent is read somewhere:\n{text}"
        );
        // Every line reading the tangent is inside a `#ifdef VERTEX_TANGENTS`
        // that has not been closed yet.
        let mut guarded = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "#ifdef VERTEX_TANGENTS" {
                guarded = true;
            } else if trimmed == "#endif" || trimmed == "#else" {
                guarded = trimmed == "#else" && guarded;
            }
            assert!(
                guarded || !line.contains("world_tangent"),
                "{stage:?} reads a tangent outside the def that declares one:\n{text}"
            );
        }
    }
}

#[test]
#[ignore = "prints the generated text for reading, rather than asserting about it"]
fn print_the_library_brick() {
    let library = shipped();
    let material = library.build("library:brick").unwrap();
    let split = partition(&material, &everything(&material), 512).unwrap();
    println!("{}", split.report());
    println!("{}", emit(&split, Stage::Prepass).unwrap().module());
}

// -------------------------------------------- the roughness widens with distance

/// A wall with a bound normal map and a roughness, live or not, as the caller
/// asks: the two shapes the widening has to hold for.
fn wall(roughness: &str, strength: f32) -> Partition {
    shifted(|builder| {
        builder
            .node("grain", Noise::value().period(8))
            .node("rough", Math::new(MathOp::Mul, "grain", 0.8))
            .node(
                "wet",
                Math::new(MathOp::Mul, "grain", Input::param("shift")),
            )
            .output(
                PbrOutput::new()
                    .base_color("grain")
                    .roughness(roughness)
                    .height("grain")
                    .normal_strength(strength),
            )
    })
}

#[test]
fn a_static_normal_and_a_static_roughness_widen_with_distance() {
    // The claim: a compiled material does what a baked one does at a distance.
    // `mips::encode_mips` widens the roughness of every level by how short the
    // mean of that level's normals got, and can only do it because a bake holds
    // both planes at once. A partition binds separate images, so the coherence
    // rides in the alpha of the normal map's own chain and the fragment applies
    // the same formula to whatever level the hardware chose.
    //
    // The static-roughness case is the one that could have been handled by
    // widening the bound roughness map's own chain at bake time instead. This
    // is the shader doing it, so that the live case below is the same code.
    let split = wall("rough", 0.02);
    assert!(split.report().widens);
    let core = emit(&split, Stage::Fragment).unwrap().core().to_owned();
    // The coherence is the alpha of the normal map, at the coordinate the
    // normal map is read at, and it is a read the shader was making anyway.
    assert!(
        core.contains("let ashlar_coherence: f32 = textureSample("),
        "{core}"
    );
    assert!(core.contains(".a;\n"), "{core}");
    // The bake's own formula, with the crate's own constant: nothing here is
    // allowed to be a second Toksvig curve that happens to look similar.
    assert!(
        core.contains(&format!(
            "(1.0 - ashlar_coherence) * {:?}f), 1.0);",
            ashlar_material::mips::TOKSVIG_K
        )),
        "{core}"
    );
    assert!(core.contains("surface.roughness = min(sqrt("), "{core}");
    validate_every_flavour(&split, "a wall that widens with distance");
}

#[test]
fn a_live_roughness_widens_the_same_way() {
    // The case route (B) would have done nothing for, and the reason the
    // widening is in the fragment rather than in the bound roughness map's
    // chain: the study's wet concrete computes its roughness per fragment out
    // of a uniform, so there is no roughness map to have widened.
    let split = wall("wet", 0.02);
    assert!(split.report().widens);
    let core = emit(&split, Stage::Fragment).unwrap().core().to_owned();
    assert!(core.contains("ashlar_coherence"), "{core}");
    // The widened value is the live expression, not a texture read: the term is
    // applied after the runtime roughness rather than instead of it.
    let live = split.runtime().root("wet");
    assert!(live.is_none(), "the port is `roughness`, not the node name");
    assert!(core.contains("ashlar.shift"), "{core}");
    validate_every_flavour(&split, "a wall whose live roughness widens");
}

#[test]
fn a_material_whose_normal_is_live_says_its_roughness_does_not_widen() {
    // Two ways to have no `|n_avg|` to read, and they are not the same thing.
    //
    // A graph with `normal_strength` at zero writes no `normal` output at all,
    // so its normal plane is `[0, 0, 1]` everywhere: the mean is a unit vector
    // at every level, the bake's own Toksvig term is zero at every level, and
    // both sides ship the plain box filter of the roughness. The fragment
    // widens nothing and neither does the bake, which is agreement rather than
    // a shortfall — so the report must *not* warn about it, or it cries wolf on
    // every flat-faced graph in the study.
    let flat = wall("rough", 0.0);
    assert!(
        flat.outputs().get("normal").is_none(),
        "{:?}",
        flat.outputs()
    );
    assert!(flat.widening().is_none(), "there is no normal map to read");
    assert!(
        flat.report().widens,
        "a flat normal plane agrees with the bake: {}",
        flat.report()
    );
    assert!(
        !flat
            .report()
            .to_string()
            .contains("roughness does not widen with distance"),
        "{}",
        flat.report()
    );
    assert!(
        !emit(&flat, Stage::Fragment)
            .unwrap()
            .core()
            .contains("ashlar_coherence")
    );

    // A height that moves with a live parameter is four taps of that parameter
    // per fragment: a direction, and no record of what a lower level averaged
    // away. Here the bake *would* widen and the fragment cannot, which is the
    // one real difference — and the case the README and the report both name.
    let live_height = shifted(|builder| {
        builder
            .node("grain", Noise::value().period(8))
            .node(
                "lift",
                Math::new(MathOp::Mul, "grain", Input::param("shift")),
            )
            .output(
                PbrOutput::new()
                    .roughness("grain")
                    .height("lift")
                    .normal_strength(0.02),
            )
    });
    assert!(!live_height.report().widens, "{}", live_height.report());
    assert!(
        live_height
            .report()
            .to_string()
            .contains("roughness does not widen with distance"),
        "{}",
        live_height.report()
    );
    validate_every_flavour(&live_height, "a wall whose normal is live");
}

#[test]
fn a_compute_kernel_reads_the_level_it_was_asked_for() {
    // A fragment picks its mip level from derivatives a dispatch does not have,
    // so the only way to ask a compiled material what it looks like from
    // further away is to name the level. At level 0 the widening is exactly
    // nothing — the coherence of unit normals is one — which is why measuring
    // it needs this.
    use ashlar_material::wgsl::{MeshInputs, emit_compute_mesh};
    let split = wall("rough", 0.02);
    let ports: Vec<String> = split.outputs().keys().cloned().collect();
    let fetches = |module: &str, level: u32| {
        let reads = module.matches("textureSampleLevel(").count();
        let at_level = module.matches(&format!(", {level}.0)")).count();
        assert!(reads > 0, "{module}");
        assert_eq!(
            reads, at_level,
            "not every read is at level {level}\n{module}"
        );
    };
    let level0 = emit_compute_mesh(&split, 256, &ports, MeshInputs::Uniform, 0).unwrap();
    assert_eq!(level0.level(), 0);
    let level3 = emit_compute_mesh(&split, 32, &ports, MeshInputs::Uniform, 3).unwrap();
    assert_eq!(level3.level(), 3);
    fetches(level3.module(), 3);
    validate(level3.module(), "a kernel reading level 3");

    // A level the chain does not have is a kernel that compiles and reads
    // whatever the view clamps to, which would be a silent wrong answer in the
    // one place whose whole job is being right. The square being dispatched is
    // what says which levels exist, so this is refused on the pair: level 3 of
    // a 32-texel square is level 0 of a 256-texel chain and fine, while level
    // 30 of it is level 0 of a chain no material has.
    let past_the_end = emit_compute_mesh(&split, 32, &ports, MeshInputs::Uniform, 30);
    let error = past_the_end.expect_err("a level past the chain is refused");
    assert!(error.to_string().contains("level 30"), "{error}");
}

#[test]
fn generated_normal_stages_convert_uv_rows_to_bevy_opengl_basis() {
    let split = shifted(|b| {
        b.node(
            "h",
            Math::new(
                MathOp::Mul,
                Noise::perlin().period(4),
                Input::param("shift"),
            ),
        )
        .output(PbrOutput::new().height("h").normal_strength(0.02))
    });
    for stage in [Stage::Fragment, Stage::Prepass] {
        let shader = emit(&split, stage).unwrap();
        assert!(
            shader
                .module()
                .contains("surface.normal * vec3<f32>(1.0, -1.0, 1.0)")
        );
    }
}
