//! Properties of validation, over graphs assembled at random from the vocabulary.
//!
//! The example-based tests fix the cases that were once wrong. This one fixes
//! the invariant that matters for a file format: validation is a predicate over
//! authored data, so it has to answer for anything a file or an editor can
//! hold, including references to nothing, cycles, NaN and a node wired to
//! itself. Case counts stay modest: this runs on every `cargo test`.
//!
//! The last property is about lowering rather than validation, and it is here
//! because what it claims is about validation: a material that built is one a
//! backend can walk, so the generator that reaches every wiring is the thing
//! that can say so.
#![allow(
    clippy::unwrap_used,
    reason = "generated fixtures; a failure is a test failure"
)]
use ashlar_material::{
    BlendMode, Channel, GraphParam, Input, MaterialGraph, MaterialGraphLibrary, MathOp, MirrorAxis,
    Node, Param, ParamValue, PbrOutput, Period, SdfOp, ShapeKind, ShapeOutput, SurfaceOutput,
    Value,
    ir::{Target, lower},
    nodes::{
        Adjust, Blend, Blur, Bricks, Buffer, CircleMap, CircleSplatter, Clamp, Colorize, Combine,
        Comment, Curvature, Curve, Decompose, Dilate, DirectionalWarp, Distance, EdgeDetect, Erode,
        GraphInput, HeightToMask, Invert, Kaleidoscope, Levels, Math, Mirror, Mix, Noise,
        NormalFromHeight, OcclusionFromHeight, Pattern, PatternKind, Scratches, SdfCombine,
        SdfMask, Shape, Subgraph, Switch, Tile, TilePattern, Tiles, Transform, Triplanar, Uv,
        Voronoi, Warp, Weave, WeaveOutput, WeavePattern, WorldAxis, WorldField, WorldMask,
    },
};
use proptest::{prelude::*, strategy::Union};

/// The four ids a generated graph uses, in the order it declares them.
const IDS: [&str; 4] = ["a", "b", "c", "d"];

/// What an editor or a hand-written file can put in a reference besides a
/// name: nothing at all, whitespace, the id an inline input would generate,
/// and a name no node has.
const ODD_IDS: [&str; 4] = ["", " ", "c.input", "missing"];

/// Parameter names a graph may refer to. Only two of them exist.
const PARAMS: [&str; 3] = ["wear", "tint", "absent"];

/// The library a generated graph is built against.
///
/// Four keys, one per shape a [`Subgraph`] can take: `a` resolves and has one
/// parameter, `b` resolves through another graph, `c` includes itself, and `d`
/// is in no library at all. A generated subgraph names one of `IDS`, so every
/// one of those four answers is reached under random wiring rather than only in
/// the example tests.
///
/// `a` also declares a signal input, `bias`, and reaches its own output through
/// it. That is what puts the binding half of resolution under random wiring: a
/// name the graph does not declare, a type it does not admit, and a field it
/// does admit all arrive here, and each has to be answered rather than
/// panicked at. Its default is one and the multiply is by that default, so an
/// unbound instance is the graph it always was, which is what keeps the
/// example-based expectations below about `a` true. It exports a mask beside
/// its channels for the same reason: an auxiliary output is a name a caller
/// can read or misspell, and both answers belong under random wiring.
fn library() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("a")
            .param(Param::float("wear", 0.5))
            .node("noise", Noise::value().period(3))
            .node("bias", GraphInput::float("bias", 1.0))
            .node("biased", Blend::new(BlendMode::Multiply, "noise", "bias"))
            .output(
                PbrOutput::new()
                    .base_color("biased")
                    .roughness(Input::param("wear"))
                    .height("biased")
                    .extra("mask", "noise"),
            )
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("b")
            .node("inner", Subgraph::new("a").output(SurfaceOutput::Height))
            .output(PbrOutput::new().base_color("inner"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("c")
            .node("inner", Subgraph::new("c"))
            .output(PbrOutput::new().base_color("inner"))
            .into_graph(),
    );
    library
}

/// Mostly the range a node parameter is meant to live in, and sometimes not.
fn scalar() -> impl Strategy<Value = f32> {
    prop_oneof![
        40 => 0.05f32..0.95,
        6 => -2.0f32..2.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
    ]
}

/// Mostly a period a generator accepts, and sometimes zero.
fn period() -> impl Strategy<Value = u32> {
    prop_oneof![20 => 1u32..18, 1 => Just(0)]
}

/// A brick bond: stack, running, third, and one that divides nothing.
///
/// Whether it closes depends on the row count, which is generated apart from
/// it, so both answers to the bond rule are reached under random wiring.
fn bond() -> impl Strategy<Value = f32> {
    prop_oneof![
        3 => Just(0.0f32),
        3 => Just(0.5),
        2 => Just(1.0 / 3.0),
        1 => Just(0.37),
    ]
}

/// A tile lattice, whose parity rules likewise depend on the counts.
fn lattice() -> impl Strategy<Value = TilePattern> {
    prop::sample::select(vec![
        TilePattern::Grid,
        TilePattern::Hex,
        TilePattern::Herringbone,
    ])
}

/// A literal, a parameter, a reference, or a node written inline.
///
/// `earlier` is what the graph has declared so far. A reference into it is the
/// common case, which is what keeps most generated graphs acyclic and most of
/// them therefore reaching the far side of validation; the rest of the weight
/// is on the references that are a node's own id, a name no node has, or no
/// name at all.
fn input(earlier: &'static [&'static str]) -> BoxedStrategy<Input> {
    let mut options: Vec<(u32, BoxedStrategy<Input>)> = vec![
        (4, scalar().prop_map(Input::float).boxed()),
        (
            1,
            (scalar(), scalar(), scalar())
                .prop_map(|(r, g, b)| Input::color([r, g, b]))
                .boxed(),
        ),
        (
            1,
            (scalar(), scalar())
                .prop_map(|(u, v)| Input::vec2([u, v]))
                .boxed(),
        ),
        (
            2,
            prop_oneof![
                8 => prop::sample::select(PARAMS[..2].to_vec()),
                1 => Just(PARAMS[2]),
            ]
            .prop_map(Input::param)
            .boxed(),
        ),
        (
            1,
            leaf()
                .prop_map(|node| Input::Inline(Box::new(node)))
                .boxed(),
        ),
        (
            2,
            prop::sample::select(IDS.iter().chain(&ODD_IDS).copied().collect::<Vec<_>>())
                .prop_map(Input::node)
                .boxed(),
        ),
    ];
    if !earlier.is_empty() {
        options.push((
            12,
            prop::sample::select(earlier).prop_map(Input::node).boxed(),
        ));
    }
    Union::new_weighted(options).boxed()
}

/// A shape of any kind, with every one of its five lengths drawn from the whole
/// of [`scalar()`].
///
/// Split out of [`leaf`] for length. A corner radius wider than the shape, a
/// capsule longer than the repeat, a gear cut deeper than its own radius and
/// NaN in any of them all reach validation here, which has to answer rather
/// than run.
fn shape() -> BoxedStrategy<Node> {
    (
        prop::sample::select(vec![
            ShapeKind::Circle,
            ShapeKind::Box,
            ShapeKind::Polygon,
            ShapeKind::Star,
            ShapeKind::Capsule,
            ShapeKind::Gear,
        ]),
        scalar(),
        scalar(),
        scalar(),
        scalar(),
        scalar(),
        prop::sample::select(vec![ShapeOutput::Mask, ShapeOutput::Distance]),
    )
        .prop_map(|(kind, size, round, hollow, length, depth, output)| {
            Shape::new(kind)
                .size(size)
                .round(round)
                .hollow(hollow)
                .length(length)
                .depth(depth)
                .output(output)
                .into()
        })
        .boxed()
}

/// A cloth, drawn over the whole of every field it takes.
///
/// Split out of [`leaf`] for length, the way [`shape`] is. What this reaches
/// that the tiling set cannot is the crossing rules that have no answer: a
/// thread count the pattern does not divide, a twill of two threads, a satin
/// of four or six whose repeat admits no move of its own, a zero step that
/// would divide by nothing, and a thread wider than its own pitch. Every one
/// of them has to come back as a path rather than as a panic or a cloth with a
/// fault down it.
fn weave() -> BoxedStrategy<Node> {
    (
        prop_oneof![
            1 => Just(WeavePattern::Plain),
            1 => period().prop_map(|step| WeavePattern::Twill { step }),
            1 => period().prop_map(|step| WeavePattern::Satin { step }),
        ],
        period(),
        period(),
        scalar(),
        prop::sample::select(vec![
            WeaveOutput::Mask,
            WeaveOutput::Height,
            WeaveOutput::Warp,
            WeaveOutput::Weft,
            WeaveOutput::Id,
        ]),
    )
        .prop_map(|(pattern, x, y, width, output)| {
            Weave::new()
                .x(x)
                .y(y)
                .width(width)
                .pattern(pattern)
                .output(output)
                .into()
        })
        .boxed()
}

/// A node with no wired inputs, for the inline case and for the generators.
fn leaf() -> impl Strategy<Value = Node> + Clone {
    prop_oneof![
        3 => Just(Node::from(Uv::new())),
        6 => (period(), 1u32..4, 1u32..4).prop_map(|(period, octaves, lacunarity)| Noise::value()
            .period(period)
            .octaves(octaves)
            .lacunarity(lacunarity)
            .into()),
        6 => (period(), period()).prop_map(|(u, v)| Noise::perlin().periods(u, v).into()),
        3 => (period(), scalar()).prop_map(|(period, jitter)| Voronoi::new()
            .period(period)
            .jitter(jitter)
            .into()),
        3 => (period(), period(), scalar(), bond()).prop_map(|(rows, columns, mortar, offset)| {
            Bricks::new()
                .rows(rows)
                .columns(columns)
                .mortar(mortar)
                .offset(offset)
                .into()
        }),
        3 => (period(), period(), lattice()).prop_map(|(rows, columns, pattern)| Tiles::new()
            .pattern(pattern)
            .rows(rows)
            .columns(columns)
            .into()),
        3 => (period(), period())
            .prop_map(|(x, y)| Pattern::new(PatternKind::Checker).x(x).y(y).into()),
        3 => shape(),
        3 => weave(),
        3 => (1u32..64, scalar()).prop_map(|(count, width)| Scratches::new()
            .count(count)
            .width(width)
            .into()),
        2 => Just(Node::from(Comment::new("note"))),
        // An input is a leaf the way a generator is: it declares what it
        // carries and reads nothing. Two of them drawing one name is the
        // duplicate refusal, and a blank one is the name rule every id
        // follows, so both are drawn from.
        2 => (
            prop_oneof![
                8 => prop::sample::select(vec!["bias", "wear"]),
                1 => Just(" "),
            ],
            prop_oneof![
                scalar().prop_map(Value::Float),
                (scalar(), scalar(), scalar()).prop_map(|(r, g, b)| Value::Color([r, g, b])),
                (scalar(), scalar()).prop_map(|(u, v)| Value::Vec2([u, v])),
            ],
        )
            .prop_map(|(name, default)| match default {
                Value::Color(color) => GraphInput::color(name, color).into(),
                Value::Vec2(uv) => GraphInput::vec2(name, uv).into(),
                Value::Float(float) => GraphInput::float(name, float).into(),
            }),
        // A world mask is a leaf like the generators, and the one leaf a bake
        // refuses: the lowering property below is what says so.
        3 => (
            prop::sample::select(vec![WorldField::Normal, WorldField::Position]),
            prop::sample::select(vec![WorldAxis::Y, WorldAxis::NegY, WorldAxis::NegZ]),
            scalar(),
            scalar(),
        )
            .prop_map(|(field, axis, threshold, softness)| WorldMask::up()
                .field(field)
                .axis(axis)
                .threshold(threshold)
                .softness(softness)
                .into()),
        // Drawn twice as often as the rest of the leaves carry their weight,
        // because three in four of these bind a field and the binding is the
        // half of subgraph resolution the generator is here to reach. Mostly
        // one of the two keys that resolve, for the same reason: the missing
        // key and the recursive one are answered wherever they are drawn, and
        // a graph refused at the key never reaches the binding at all.
        2 => (
            prop_oneof![
                3 => prop::sample::select(IDS[..2].to_vec()),
                1 => prop::sample::select(IDS[2..].to_vec()),
            ],
            prop::sample::select(vec![
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Height,
                SurfaceOutput::Emissive,
                SurfaceOutput::Extra("mask".to_owned()),
            ]),
            prop::sample::select(PARAMS.as_slice()),
            scalar(),
            // A field bound into an input, three times in four. Mostly the
            // name `a` declares and mostly a type its float input admits,
            // because the answer worth reaching often is the one where the
            // instance is really specialised; the name it does not declare and
            // the `Vec2` that converts to nothing are the two refusals, drawn
            // rarely rather than never.
            prop::option::weighted(0.75, (
                prop_oneof![3 => Just("bias"), 1 => Just("absent")],
                prop_oneof![
                    4 => scalar().prop_map(Input::float),
                    2 => (scalar(), scalar(), scalar())
                        .prop_map(|(r, g, b)| Input::color([r, g, b])),
                    1 => (scalar(), scalar()).prop_map(|(u, v)| Input::vec2([u, v])),
                ],
            )),
        )
            .prop_map(|(graph, output, param, value, bound)| {
                let mut node = Subgraph::new(graph)
                    .output(output)
                    .param(param, ParamValue::Float(value));
                if let Some((name, input)) = bound {
                    node = node.input(name, input);
                }
                node.into()
            }),
    ]
}

/// The resamplers, which read their source somewhere other than the texel
/// asking. Split out of [`node`] for length, and split again from
/// [`buffered`] for the same reason; the weights are branch counts, so a node
/// is drawn as often as it would be had the three never been split.
fn resampled(earlier: &'static [&'static str]) -> BoxedStrategy<Node> {
    prop_oneof![
        1 => (input(earlier), scalar(), scalar()).prop_map(|(input, scale, rotate)| Transform::new(
            input
        )
        .scale(scale)
        .rotate(rotate)
        .into()),
        1 => (input(earlier), input(earlier), 1u32..8)
            .prop_map(|(input, mask, count)| Tile::new(input).mask(mask).count(count).into()),
        1 => (input(earlier), input(earlier), scalar())
            .prop_map(|(s, o, amount)| Warp::new(s, o).amount(amount).into()),
        1 => input(earlier).prop_map(|input| Mirror::new(input).axis(MirrorAxis::V).into()),
        1 => (input(earlier), 0u32..8)
            .prop_map(|(input, count)| Kaleidoscope::new(input).count(count).into()),
        // The radius, the hole and the two counts are drawn past their bounds
        // as well: a disc that reaches the seam, a hole outside the window it
        // is in and a count of zero are each refused at the field that asked,
        // and a validation property is where that has to answer rather than
        // run.
        1 => (
            input(earlier),
            scalar(),
            scalar(),
            period(),
            period(),
            scalar(),
        )
            .prop_map(
                |(input, radius, inner, turns, rings, twist)| CircleMap::new(input)
                    .radius(radius)
                    .inner(inner)
                    .turns(turns)
                    .rings(rings)
                    .twist(twist)
                    .into()
            ),
        // The radius, the standoff, the instance width and all four variations
        // are drawn past their bounds as well: a ring that reaches the seam, a
        // standoff outside the ring it stands in, an instance of no width and a
        // variation past one are each refused at the field that asked, and a
        // validation property is where that has to answer rather than run.
        1 => (
            input(earlier),
            input(earlier),
            (scalar(), scalar(), scalar()),
            (scalar(), scalar(), scalar(), scalar()),
            (period(), period(), any::<bool>()),
        )
            .prop_map(
                |(input, mask, (radius, inner, scale), variation, (slots, rings, facing))| {
                    let mut node = CircleSplatter::new(input)
                        .mask(mask)
                        .count(slots)
                        .rings(rings)
                        .radius(radius)
                        .inner(inner)
                        .scale(scale)
                        .scale_variation(variation.0)
                        .rotation_variation(variation.1)
                        .radius_variation(variation.2)
                        .opacity_variation(variation.3);
                    if facing {
                        node = node.face_centre();
                    }
                    node.into()
                },
            ),
        11 => buffered(earlier),
    ]
    .boxed()
}

/// The filters that rasterise a plane and read it back, and the two nodes that
/// address their source by something other than UV. Split out of [`resampled`]
/// for length.
fn buffered(earlier: &'static [&'static str]) -> BoxedStrategy<Node> {
    prop_oneof![
        (input(earlier), scalar())
            .prop_map(|(input, radius)| Blur::new(input).radius(radius).into()),
        (input(earlier), scalar(), scalar()).prop_map(|(input, radius, angle)| Blur::directional(
            input, angle
        )
        .radius(radius)
        .into()),
        // The step count is drawn past the bound as well: a slope blur that
        // asked for a thousand walks is refused at `steps`, and a validation
        // property is exactly where that has to answer rather than run.
        (input(earlier), input(earlier), scalar(), 0_u32..24).prop_map(
            |(input, height, radius, steps)| Blur::slope(input, height)
                .radius(radius)
                .steps(steps)
                .into()
        ),
        (input(earlier), scalar(), scalar()).prop_map(|(height, radius, strength)| {
            Curvature::cavity(height)
                .radius(radius)
                .strength(strength)
                .into()
        }),
        (input(earlier), scalar(), scalar()).prop_map(|(input, radius, strength)| EdgeDetect::new(
            input
        )
        .radius(radius)
        .strength(strength)
        .into()),
        (input(earlier), scalar())
            .prop_map(|(h, radius)| OcclusionFromHeight::new(h).radius(radius).into()),
        (input(earlier), scalar())
            .prop_map(|(input, range)| Distance::new(input).range(range).into()),
        input(earlier).prop_map(|input| Erode::new(input).into()),
        input(earlier).prop_map(|input| Dilate::new(input).into()),
        (
            input(earlier),
            prop_oneof![6 => prop::sample::select(vec![64u32, 256, 1024]), 1 => Just(300)],
        )
            .prop_map(|(input, resolution)| Buffer::new(input).resolution(resolution).into()),
        // The tiling and the sharpness are drawn past their bounds as well: a
        // triplanar over zero metres and one at a sharpness of a half are both
        // refused at the field that asked, and a validation property is where
        // that has to answer rather than run.
        (input(earlier), scalar(), scalar()).prop_map(|(source, metres, sharpness)| {
            Triplanar::new(source)
                .tile_metres(metres)
                .sharpness(sharpness * 8.0)
                .into()
        }),
    ]
    .boxed()
}

/// Anything in the vocabulary, wired to anything at all.
fn node(earlier: &'static [&'static str]) -> BoxedStrategy<Node> {
    let wired = prop_oneof![
        (input(earlier), input(earlier), input(earlier)).prop_map(|(a, b, o)| Blend::new(
            BlendMode::Overlay,
            a,
            b
        )
        .opacity(o)
        .into()),
        (input(earlier), scalar(), scalar()).prop_map(|(input, low, gamma)| Node::from(
            Levels::new(input).in_low(low).gamma(gamma)
        )),
        (input(earlier), scalar()).prop_map(|(input, x)| Curve::new(input)
            .points([[0.0, 0.0], [x, 0.5], [1.0, 1.0]])
            .into()),
        (input(earlier), scalar()).prop_map(|(input, stop)| Colorize::new(input)
            .gradient([(0.0, [0.0; 3]), (stop, [1.0; 3])])
            .into()),
        (input(earlier), scalar()).prop_map(|(input, hue)| Adjust::new(input).hue(hue).into()),
        (input(earlier), input(earlier)).prop_map(|(a, b)| Math::new(MathOp::Atan2, a, b).into()),
        input(earlier).prop_map(|input| Decompose::new(input, Channel::B).into()),
        (input(earlier), input(earlier), input(earlier))
            .prop_map(|(r, g, b)| Combine::new(r, g, b).into()),
        input(earlier).prop_map(|input| Invert::new(input).into()),
        (input(earlier), input(earlier), input(earlier))
            .prop_map(|(i, l, h)| Clamp::new(i).range(l, h).into()),
        (input(earlier), input(earlier), input(earlier))
            .prop_map(|(a, b, t)| Mix::new(a, b, t).into()),
        (input(earlier), input(earlier), input(earlier))
            .prop_map(|(c, t, f)| Switch::new(c, t, f).into()),
        (input(earlier), scalar()).prop_map(|(h, s)| NormalFromHeight::new(h).strength(s).into()),
        (input(earlier), scalar(), scalar(), scalar()).prop_map(|(i, low, high, softness)| {
            HeightToMask::band(i, low, high).softness(softness).into()
        }),
        // The fillet and the ramp are widths in UV, drawn past their bounds as
        // well: a negative one, a NaN and a width wider than the repeat are
        // each refused at the field that asked, and a validation property is
        // where that has to answer rather than run.
        (
            prop::sample::select(vec![SdfOp::Union, SdfOp::Intersect, SdfOp::Subtract]),
            input(earlier),
            input(earlier),
            scalar(),
        )
            .prop_map(|(op, a, b, smooth)| SdfCombine::new(op, a, b).smooth(smooth).into()),
        (input(earlier), scalar()).prop_map(|(input, edge)| SdfMask::new(input).edge(edge).into()),
        (input(earlier), input(earlier), scalar()).prop_map(|(s, a, amount)| DirectionalWarp::new(
            s, a
        )
        .amount(amount)
        .into()),
    ];
    prop_oneof![wired, resampled(earlier), leaf()].boxed()
}

/// A whole graph: two parameters, four nodes wired in declaration order, and
/// an output bound to whatever happened to be there.
fn graph() -> impl Strategy<Value = MaterialGraph> {
    (
        node(&[]),
        node(&IDS[..1]),
        node(&IDS[..2]),
        node(&IDS[..3]),
        (input(&IDS), input(&IDS), input(&IDS), input(&IDS), scalar()),
    )
        .prop_map(|(a, b, c, d, output)| {
            let (base_color, roughness, height, emissive, strength) = output;
            let mut builder = MaterialGraph::builder("test:generated")
                .param(Param::float("wear", 0.5).range(0.0, 1.0))
                .param(Param::color("tint", [0.5; 3]));
            for (id, node) in IDS.into_iter().zip([a, b, c, d]) {
                builder = builder.node(id, node);
            }
            builder
                .output(
                    PbrOutput::new()
                        .base_color(base_color)
                        .roughness(roughness)
                        .height(height)
                        .emissive(emissive)
                        .normal_strength(strength),
                )
                .into_graph()
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Validation answers for anything a file can hold. Nothing here may
    /// panic, overflow, recurse without end, or loop forever.
    #[test]
    fn validation_answers_rather_than_panicking(graph in graph()) {
        let library = library();
        let answer = graph.clone().build_in(&library);
        // And it answers the same way twice: nothing here depends on
        // iteration order or on an address.
        prop_assert_eq!(answer.is_ok(), graph.clone().build_in(&library).is_ok());
        match answer {
            Err(error) => {
                prop_assert!(!error.path.is_empty(), "an error without a path");
                prop_assert!(!error.reason.is_empty());
                prop_assert!(error.to_string().contains(&error.path));
            }
            Ok(material) => {
                // A material that built is one a backend can walk: the output
                // tiles, every port it names resolves, and dependencies come
                // before the nodes that read them.
                prop_assert!(material.period().is_tiled());
                let mut seen = Vec::new();
                for id in material.order() {
                    let node = &material.graph().nodes[id];
                    for port in node.inputs() {
                        if let Input::Node(target) = port.input {
                            prop_assert!(seen.contains(&target), "{target} came after {id}");
                        }
                        prop_assert!(
                            material.port_of(port.input).is_some(),
                            "{id}.{} does not resolve",
                            port.name,
                        );
                    }
                    seen.push(id);
                }
                for port in material.output_ports().values() {
                    prop_assert!(port.period.is_tiled());
                }
            }
        }
    }

    /// Lowering is total over materials that built, bar two named refusals.
    ///
    /// One is a node whose lowering has not landed. The other is a *target*
    /// refusing a node it cannot answer for: a world position, a world normal,
    /// and the triplanar and world mask built out of them have no value in a
    /// bake, so this target refuses them by node path and names the surface
    /// that does answer. Anything else — most of all a conversion — would be a
    /// graph validation accepted and no backend can run, which is what a
    /// `Color` reaching a displacement port was: the port admits whatever
    /// converts to a float, and the IR had no way to spend one on two axes.
    #[test]
    fn a_material_that_builds_lowers_or_names_the_node_it_cannot_lower(graph in graph()) {
        // A graph that did not validate is the property above; most random
        // wirings are one, so this skips them rather than rejecting the case.
        let Ok(material) = graph.build_in(&library()) else {
            return Ok(());
        };
        if let Err(error) = lower(&material, Target::Bake) {
            let refused = error.reason.ends_with("has no lowering yet")
                || error.reason.contains("deliver this material as a Shader surface");
            prop_assert!(refused, "lowering refused a validated material: {error}");
            prop_assert!(error.path.starts_with("nodes["), "{error}");
            // And the same graph lowers for the target that can answer it.
            if !error.reason.ends_with("has no lowering yet") {
                let live: Vec<String> = Vec::new();
                prop_assert!(lower(&material, Target::Shader { live }).is_ok(), "{error}");
            }
        }
    }

    /// Serde reads exactly what the builder writes, for any graph the builder
    /// can make. RON is the interchange form, so this is the file format.
    #[test]
    fn any_graph_round_trips_through_ron(graph in graph()) {
        let encoded = ron::to_string(&graph).unwrap();
        // A NaN is not equal to itself, so a round trip cannot be compared by
        // value; validation rejects those anyway, by path.
        prop_assume!(!encoded.contains("NaN"));
        let decoded: MaterialGraph = ron::from_str(&encoded).unwrap();
        prop_assert_eq!(&decoded, &graph);
        let library = library();
        prop_assert_eq!(decoded.build_in(&library).is_ok(), graph.build_in(&library).is_ok());
    }

    /// A pointwise node repeats a whole number of times for each of its
    /// inputs: that is what a least common multiple is for, and it is the
    /// property a bake relies on when it wraps a blend of two lattices.
    #[test]
    fn a_blend_repeats_a_whole_number_of_times_for_each_of_its_inputs(
        left in 1u32..24,
        right in 1u32..24,
        constant in -1.0f32..1.0,
    ) {
        let material = MaterialGraph::builder("test:lcm")
            .node("left", Noise::value().period(left))
            .node("right", Noise::perlin().period(right))
            .node("mix", Blend::new(BlendMode::Add, "left", "right").opacity(constant))
            .output(PbrOutput::new().base_color("mix"))
            .build()
            .unwrap();
        let Some([u, v]) = material.port("mix").unwrap().period.repeats() else {
            return Err(TestCaseError::fail("a blend of two lattices tiles"));
        };
        prop_assert_eq!(u, v);
        prop_assert_eq!(u % left, 0, "{} is not a multiple of {}", u, left);
        prop_assert_eq!(u % right, 0, "{} is not a multiple of {}", u, right);
        // And it is the smallest such number, or every graph inflates.
        prop_assert!((1..u).all(|smaller| smaller % left != 0 || smaller % right != 0));
    }

    /// Every shape a subgraph node can take gets the answer it deserves: the
    /// period of the output it read, the key that is missing, the ring it
    /// closes, or the parameter the instanced graph does not have.
    ///
    /// The generator above reaches subgraphs rarely and mostly with the rest of
    /// the graph already broken, so resolution gets its own property here.
    #[test]
    fn a_subgraph_resolves_recurses_or_says_which_key_or_parameter_is_missing(
        key in prop::sample::select(IDS.as_slice()),
        output in prop::sample::select(vec![
            SurfaceOutput::BaseColor,
            SurfaceOutput::Roughness,
            SurfaceOutput::Height,
            SurfaceOutput::Emissive,
        ]),
        binding in prop::option::of(prop::sample::select(PARAMS.as_slice())),
    ) {
        let mut node = Subgraph::new(key).output(output.clone());
        if let Some(name) = binding {
            node = node.param(name, ParamValue::Float(0.25));
        }
        let built = MaterialGraph::builder("test:instanced")
            .node("inner", node)
            .output(PbrOutput::new().base_color("inner"))
            .build_in(&library());
        // Only `a` and `b` exist; only `a` has a parameter, and only `a` binds
        // a height. `b` reads `a`, so a resolved period came through two graphs.
        let bound = matches!((key, &output), ("a", SurfaceOutput::BaseColor | SurfaceOutput::Roughness | SurfaceOutput::Height)
            | ("b", SurfaceOutput::BaseColor | SurfaceOutput::Roughness));
        let binds_parameter = binding.is_none_or(|name| key == "a" && name == "wear");
        match built {
            Ok(material) => {
                prop_assert!(bound && binds_parameter, "{key} {output:?} {binding:?}");
                let port = material.port("inner").unwrap();
                prop_assert_eq!(port.value_type, output.value_type());
                // Both graphs put a parameter or a constant in roughness, and
                // a noise of period three everywhere else.
                let expected = if output == SurfaceOutput::Roughness {
                    Period::UNIT
                } else {
                    Period::square(3)
                };
                prop_assert_eq!(port.period, expected);
                prop_assert_eq!(material.period(), expected);
            }
            Err(error) => {
                let (path, reason) = match key {
                    "c" => ("graphs[c].nodes[inner].graph".to_owned(), "recursive subgraph"),
                    "d" => ("nodes[inner].graph".to_owned(), "unknown graph"),
                    _ if !binds_parameter => (
                        format!("nodes[inner].params[{}]", binding.unwrap()),
                        "has no parameter",
                    ),
                    _ => ("nodes[inner].output".to_owned(), "binds no"),
                };
                prop_assert_eq!(&error.path, &path);
                prop_assert!(error.reason.contains(reason), "{}", error);
            }
        }
    }

    /// Every parameter type reaches the graph as a value of its own port type,
    /// whatever a file put in it.
    #[test]
    fn a_parameter_widens_into_the_graph_without_losing_its_type(
        value in prop_oneof![
            scalar().prop_map(ParamValue::Float),
            (scalar(), scalar(), scalar()).prop_map(|(r, g, b)| ParamValue::Color([r, g, b])),
            any::<i32>().prop_map(ParamValue::Int),
            any::<bool>().prop_map(ParamValue::Bool),
        ],
    ) {
        prop_assert_eq!(value.value().value_type(), value.value_type());
        prop_assert_eq!(value.is_finite(), value.value().is_finite());
        if let Value::Float(float) = value.value() {
            prop_assert!(!matches!(value, ParamValue::Bool(_)) || (0.0..=1.0).contains(&float));
        }
    }
}

/// The one period the lcm rule has to get right at its edges, checked without
/// generating a graph for it.
#[test]
fn the_neutral_and_absorbing_elements_of_a_period() {
    assert_eq!(Period::UNIT.lcm(Period::UNIT), Period::UNIT);
    assert_eq!(Period::UNIT.lcm(Period::Free), Period::Free);
    assert_eq!(Period::Free.lcm(Period::square(4)), Period::Free);
    assert_eq!(Period::square(4).lcm(Period::square(6)), Period::square(12));
    assert_eq!(
        Period::Tiled { u: 4, v: 1 }.lcm(Period::Tiled { u: 1, v: 6 }),
        Period::Tiled { u: 4, v: 6 }
    );
    assert_eq!(Period::square(4).repeats(), Some([4, 4]));
    assert_eq!(Period::Free.repeats(), None);
}

/// Not a property: a count of how often the generator makes a graph that
/// actually validates, so the Ok branch above is known to be exercised, and of
/// how often it reaches the two wirings signal inputs added.
///
/// The two shapes are counted apart from the vocabulary because each of them
/// is rare on its own — an input is one leaf of a dozen, and a subgraph is
/// another — and a generator that stopped making them would leave every
/// property above still passing while saying nothing about either.
#[test]
fn the_generator_reaches_both_answers() {
    use proptest::{
        strategy::ValueTree,
        test_runner::{Config, TestRunner},
    };
    let mut runner = TestRunner::new(Config::default());
    let library = library();
    let mut ok = 0;
    let mut rejected = 0;
    let mut declares_an_input = 0;
    let mut binds_an_input = 0;
    for _ in 0..512 {
        let graph = graph().new_tree(&mut runner).unwrap().current();
        for node in graph.nodes.values() {
            match node {
                Node::GraphInput(_) => declares_an_input += 1,
                Node::Subgraph(subgraph) if !subgraph.inputs.is_empty() => binds_an_input += 1,
                _ => {}
            }
        }
        if graph.build_in(&library).is_ok() {
            ok += 1;
        } else {
            rejected += 1;
        }
    }
    // Every bar here is a fraction of what the generator actually draws — this
    // run validates about forty graphs, declares about thirty inputs and binds
    // about twenty-five fields — because what they are for is a strategy that
    // stopped reaching a shape, not the ordinary spread of a random draw. A
    // bar set at the count itself would fail a run in twenty on arithmetic
    // alone, and a flaky test is a test nobody reads.
    assert!(ok >= 16, "only {ok} of 512 generated graphs validated");
    // And the other way round: a generator that drifted into making nothing
    // but valid graphs would leave the Err branch of the property above
    // asserting nothing at all, which is the failure this half catches.
    assert!(
        rejected >= 16,
        "only {rejected} of 512 generated graphs were rejected"
    );
    assert!(declares_an_input >= 8, "only {declares_an_input} inputs");
    assert!(binds_an_input >= 8, "only {binds_an_input} bindings");
}
