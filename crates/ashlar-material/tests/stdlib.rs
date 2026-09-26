//! The shipped compounds: that each one stands on its own, and that each one
//! does what its name says.
//!
//! Two kinds of claim, and the first is the one that makes the library safe to
//! ship. Every graph here is instanced by somebody else's graph, so a compound
//! that does not tile, lays a lattice finer than a caller's bake, or warns
//! about its own periods is a fault that would otherwise surface inside a
//! building nobody was looking at. So the generic tests take *every* key
//! [`stdlib`] answers rather than a list written out here: a compound added in
//! a later step is held to the same bar without anybody remembering to add it.
//!
//! The second is behaviour, one test per compound, on a probe whose answer can
//! be worked out by hand: a step for a crest, a groove for a hollow, a flat
//! field for both, and — for the three compounds that take a *place* rather
//! than an amount — a step read as the place itself, so that every texel's
//! distance from the seed, the tear or the waterline is a row count rather
//! than a measurement. Where a compound says it changed nothing the plane must
//! be *exactly* what it was handed rather than nearly. Exactly, because
//! everything a compound does is reached through a [`Mix`] by its mask, and a
//! mask that is zero is the substrate the caller handed in, bit for bit —
//! which is the promise that instancing a compound and leaving it alone
//! changes nothing.
//!
//! The last test is not about a picture at all. It is the idiom the library
//! exists for — a wall baked into textures with one live uniform driving a
//! compound between them — and what it pins is the instruction count that
//! costs.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::float_cmp,
    reason = "a mask that did not fire is exactly zero by construction: the \
              threshold clamps at zero and every use of it is a Mix, so a \
              margin here would pass a compound that quietly moves a surface \
              it was told to leave alone"
)]

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    Channel, Input, MaterialGraph, MaterialGraphLibrary, MathOp, Param, ParamValue, PbrOutput,
    SurfaceOutput,
    bake::{BakeRequest, Planes, rasterise},
    ir::Target,
    nodes::{Decompose, Levels, Math, Noise, Subgraph, Uv, Voronoi, VoronoiOutput},
    partition::partition,
    stdlib,
};

/// Texels per repeat for every bake here.
///
/// The smallest resolution a caller is likely to ask a weathered surface for,
/// and the one the lattice claim is worth making at: a compound whose noise is
/// finer than this refuses a bake that anybody's LOD chain would make.
const RESOLUTION: u32 = 256;

/// Threads per bake, named rather than left to the default.
///
/// Eight, because the machine these tests run on faults under full
/// parallelism and a test binary is already running its cases side by side.
const THREADS: usize = 8;

/// Rasterise one graph against the standard library, with no parameters
/// overridden unless the caller passes some.
fn planes(graph: &MaterialGraph, params: &BTreeMap<String, ParamValue>) -> Planes {
    planes_at(graph, params, RESOLUTION)
}

fn planes_at(
    graph: &MaterialGraph,
    params: &BTreeMap<String, ParamValue>,
    resolution: u32,
) -> Planes {
    let library = stdlib::graphs();
    let request = BakeRequest {
        graph,
        library: &library,
        params,
        resolution,
        mips: false,
        threads: NonZeroUsize::new(THREADS),
    };
    rasterise(&request).unwrap().0
}

/// One texel of a float plane, by column and row.
fn at(plane: &[f32], x: u32, y: u32) -> f32 {
    plane[(y * RESOLUTION + x) as usize]
}

/// What [`stdlib`] ships, written out rather than read off the module.
///
/// [`stdlib`] builds its library by inserting each graph in turn, and an
/// insert of a key the library already holds replaces it silently — so a
/// builder that came to answer another's key would shrink the library by one
/// and every generic test here would keep passing over what was left. The
/// count is the one claim that catches it.
const SHIPPED: usize = 63;

#[test]
fn every_stdlib_graph_builds_alone_tiles_and_says_nothing_suspicious() {
    let library = stdlib::graphs();
    assert_eq!(
        library.graphs.len(),
        SHIPPED,
        "the standard library ships {} graphs rather than {SHIPPED}: {:?}",
        library.graphs.len(),
        library.graphs.keys().collect::<Vec<_>>()
    );
    for (key, graph) in &library.graphs {
        assert_eq!(&graph.id, key, "graph {key:?} is keyed under another id");
        let material = library
            .build(key)
            .unwrap_or_else(|error| panic!("graph {key:?} does not build alone: {error}"));
        assert!(
            material.period().is_tiled(),
            "graph {key:?} does not tile: {:?}",
            material.period()
        );
        // A warning here is almost always an lcm the author did not mean: two
        // generators whose periods do not divide make a repeat larger than
        // either, and a compound is instanced into somebody else's material,
        // whose repeat it then multiplies.
        assert_reference_warnings(key, material.warnings());
    }
}

/// The lattice a caller's own surface is laid on, for the two tests below.
///
/// Sixty-four, because every generator in this crate's own study materials is a
/// power of two and sixty-four is the one the brickwork uses; the claim being
/// made is that a shipped graph divides into it rather than multiplying it.
const HOST_CELLS: u32 = 64;

#[test]
fn every_stdlib_lattice_is_a_power_of_two() {
    let library = stdlib::graphs();
    for key in library.graphs.keys() {
        let material = library
            .build(key)
            .unwrap_or_else(|error| panic!("graph {key:?} does not build alone: {error}"));
        let repeats = material
            .period()
            .repeats()
            .unwrap_or_else(|| panic!("graph {key:?} does not tile"));
        if key.starts_with("library:soi-cobblestone") {
            assert_eq!(
                repeats,
                [512, 2560],
                "the preserved reference's five-course lattice"
            );
            continue;
        }
        if key == "library:moss-carpet" {
            assert_eq!(
                repeats,
                [768, 768],
                "the preserved reference's six-cell patch lattice"
            );
            continue;
        }
        for (axis, count) in ["u", "v"].into_iter().zip(repeats) {
            assert!(
                count.is_power_of_two(),
                "graph {key:?} repeats {count} times along {axis}, which is not a power of two: \
                 instanced onto a surface whose own lattice is a power of two, the repeat becomes \
                 the least common multiple of the two rather than the larger of them"
            );
        }
    }
}

#[test]
fn every_stdlib_graph_composes_onto_a_caller_s_own_lattice_without_multiplying_it() {
    let library = stdlib::graphs();
    for key in library.graphs.keys() {
        // The shape the library exists for: a caller's field and a shipped
        // graph's, multiplied together pointwise. Roughness because every
        // surface writes one; layouts export their mask instead. A pointwise
        // `Math` is the
        // node whose period is the least common multiple of its inputs' — the
        // place a lattice that does not divide turns into a repeat nobody
        // asked for, with a warning to say so.
        let host = MaterialGraph::builder("test:host")
            .node(
                "cells",
                Voronoi::new()
                    .period(HOST_CELLS)
                    .seed(11)
                    .output(VoronoiOutput::Cell),
            )
            .node(
                "shipped",
                Subgraph::new(key).output(if key.starts_with("layouts:") {
                    SurfaceOutput::Extra("mask".into())
                } else {
                    SurfaceOutput::Roughness
                }),
            )
            .node("both", Math::new(MathOp::Mul, "cells", "shipped"))
            .output(PbrOutput::new().roughness("both"))
            .into_graph();
        let built = host
            .build_in(&library)
            .unwrap_or_else(|error| panic!("graph {key:?} does not instance: {error}"));
        assert_reference_warnings(key, built.warnings());
        let selected = if key.starts_with("layouts:") {
            SurfaceOutput::Extra("mask".into())
        } else {
            SurfaceOutput::Roughness
        };
        let mine = MaterialGraph::builder("test:selected")
            .node("selected", Subgraph::new(key).output(selected))
            .output(PbrOutput::new().roughness("selected"))
            .into_graph()
            .build_in(&library)
            .unwrap_or_else(|error| panic!("graph {key:?} does not build alone: {error}"))
            .period()
            .repeats()
            .unwrap_or_else(|| panic!("graph {key:?} does not tile"));
        let together = built
            .period()
            .repeats()
            .unwrap_or_else(|| panic!("instancing {key:?} does not tile"));
        for (axis, (theirs, ours)) in ["u", "v"].into_iter().zip(together.into_iter().zip(mine)) {
            assert_eq!(
                theirs,
                ours.max(HOST_CELLS),
                "instancing {key:?} onto a {HOST_CELLS}-cell surface made the repeat {theirs} \
                 along {axis}, where one lattice divides the other at {}",
                ours.max(HOST_CELLS)
            );
        }
    }
}

#[test]
fn every_stdlib_graph_bakes_within_the_runtime_budget_with_height_inside_the_unit() {
    let library = stdlib::graphs();
    for (key, graph) in &library.graphs {
        let finest = library
            .build(key)
            .unwrap()
            .finest_lattice()
            .into_iter()
            .max()
            .unwrap();
        assert!(
            finest <= 512,
            "{key} exceeds the default runtime bake budget"
        );
        let resolution = RESOLUTION.max(finest).next_power_of_two();
        let planes = planes_at(graph, &BTreeMap::new(), resolution);
        assert_eq!(planes.base_color.len(), (resolution * resolution) as usize);
        // A compound binds a height only when it moves the surface, so most of
        // them have none; the one that does must keep the field a height map
        // can hold, because what leaves the unit is clipped and the normal
        // derived from it is wrong exactly where it clipped.
        if let Some(height) = &planes.height {
            for (index, value) in height.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(value),
                    "graph {key:?} bakes a height of {value} at texel {index}"
                );
            }
        }
    }
}

/// A probe that reads one compound's exported mask straight into roughness,
/// which is how a test gets at a field a bake writes no map for.
///
/// Nothing else is bound, so the plane the bake answers is the mask itself
/// rather than anything the compound did with it.
fn probe(
    key: &str,
    extra: &str,
    height: impl Into<Input>,
    params: &[(&str, f32)],
) -> MaterialGraph {
    let mut node = Subgraph::new(key)
        .input("height", height)
        .output(SurfaceOutput::Extra(extra.to_owned()));
    for (name, value) in params {
        node = node.param(*name, ParamValue::Float(*value));
    }
    MaterialGraph::builder("test:probe")
        .node("mask", node)
        .output(PbrOutput::new().roughness("mask"))
        .into_graph()
}

/// A field that is `low` below the middle of the repeat and `high` above it:
/// one riser across the whole width, with a crest along the top of it.
///
/// Built from `v` rather than from a generator so that the crest's row can be
/// named: `Step` answers one where `v >= 0.5`, so row `RESOLUTION / 2` is the
/// first row of the plateau and the one whose neighbour below is the floor.
fn step(low: f32, high: f32) -> MaterialGraph {
    MaterialGraph::builder("test:step")
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("riser", Math::new(MathOp::Step, "v", 0.5))
        .node("height", Levels::new("riser").out_range(low, high))
        .into_graph()
}

/// A narrow groove across the repeat: `low` within `half_width` of the middle,
/// `high` everywhere else.
///
/// Narrow on purpose. A hollow wider than the shelter filter's reach has open
/// sky over its middle, so the middle of a broad pit is no more sheltered than
/// a floor is; what collects dust is a hollow the surface closes over.
fn groove(low: f32, high: f32, half_width: f32) -> MaterialGraph {
    MaterialGraph::builder("test:groove")
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("into", Math::new(MathOp::Sub, "v", 0.5))
        .node("from_middle", Math::unary(MathOp::Abs, "into"))
        .node("floor", Math::new(MathOp::Step, "from_middle", half_width))
        .node("height", Levels::new("floor").out_range(low, high))
        .into_graph()
}

/// Copy the nodes of a probe field into a probe graph, under their own ids.
fn over(field: MaterialGraph, mut probe: MaterialGraph) -> MaterialGraph {
    probe.nodes.extend(field.nodes);
    probe
}

#[test]
fn edge_wear_finds_the_crest_of_a_step_and_leaves_a_flat_field_alone() {
    // An amount of one puts the threshold at zero, so the test is about what
    // the crest filter and the drag found rather than about where the default
    // threshold happens to sit.
    let worn = over(
        step(0.2, 0.8),
        probe("weathering:edge_wear", "mask", "height", &[("amount", 1.0)]),
    );
    let mask = planes(&worn, &BTreeMap::new()).roughness;
    // The first row of the plateau: the row whose neighbourhood reaches down
    // the riser, which is what standing proud means.
    let crest = at(&mask, 64, RESOLUTION / 2);
    assert!(crest > 0.0, "no wear on the crest of a step: {crest}");
    // A quarter of the way up the floor, where nothing stands proud of
    // anything.
    let floor = at(&mask, 64, RESOLUTION / 4);
    assert_eq!(floor, 0.0, "wear on the flat floor of a step");

    let flat = probe("weathering:edge_wear", "mask", 0.5, &[("amount", 1.0)]);
    let mask = planes(&flat, &BTreeMap::new()).roughness;
    for (index, value) in mask.iter().enumerate() {
        assert_eq!(*value, 0.0, "wear on a flat field at texel {index}");
    }
}

#[test]
fn dirt_dust_settles_in_a_groove_and_not_on_the_plateau() {
    let dirty = over(
        groove(0.2, 0.9, 0.01),
        probe(
            "weathering:dirt_dust",
            "cavity_mask",
            "height",
            &[("amount", 1.0)],
        ),
    );
    let mask = planes(&dirty, &BTreeMap::new()).roughness;
    let pit = at(&mask, 64, RESOLUTION / 2);
    assert!(pit > 0.0, "no dust in the groove: {pit}");
    // A tenth of the way up, five times the shelter filter's reach from the
    // groove: open sky in every direction.
    let plateau = at(&mask, 64, RESOLUTION / 10);
    assert_eq!(plateau, 0.0, "dust on the open plateau");

    let flat = probe(
        "weathering:dirt_dust",
        "cavity_mask",
        0.5,
        &[("amount", 1.0)],
    );
    let mask = planes(&flat, &BTreeMap::new()).roughness;
    for (index, value) in mask.iter().enumerate() {
        assert_eq!(*value, 0.0, "dust on a flat field at texel {index}");
    }
}

#[test]
fn a_library_refuses_a_key_it_already_holds_and_keeps_what_it_had() {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("game:wall")
            .param(Param::float("wear", 0.5))
            .into_graph(),
    );
    library.extend(&stdlib::graphs()).unwrap();
    assert!(library.get("game:wall").is_some());
    assert_eq!(library.graphs.len(), stdlib::graphs().graphs.len() + 1);

    let error = library.extend(&stdlib::graphs()).unwrap_err();
    assert!(
        error.path.starts_with("graphs["),
        "a collision should name the key: {error}"
    );
    assert_eq!(library.graphs.len(), stdlib::graphs().graphs.len() + 1);
}

/// One compound, instanced with whatever the test wires in.
///
/// The general form of [`probe`]: three of the five compounds take a *place*
/// rather than only a substrate, so the test that says what they do has to bind
/// a name of its own choosing.
fn instanced(key: &str, inputs: &[(&str, Input)], params: &[(&str, f32)]) -> Subgraph {
    let mut node = Subgraph::new(key);
    for (name, value) in inputs {
        node = node.input(*name, value.clone());
    }
    for (name, value) in params {
        node = node.param(*name, ParamValue::Float(*value));
    }
    node
}

/// How far the probe tells the rust to creep, in UV and in whole texels of
/// [`RESOLUTION`].
///
/// Five hundredths is twelve and a bit texels, which is wide enough that the
/// band has an inside and an outside with a texel of slack between them for the
/// jump flood's own quantisation. The texel count is written out rather than
/// computed so that every comparison below is between two row counts: the
/// distance plane holds a distance between texel *centres* and a row index
/// counts boundaries, and mixing the two in floating point is how a test like
/// this comes to have a tolerance nobody can explain.
const SPREAD: f32 = 0.05;
const SPREAD_TEXELS: u32 = 12;

#[test]
fn rust_reaches_its_spread_past_the_seed_and_no_further() {
    // The seed is the top half of the repeat, so every texel's distance from it
    // is a row count that can be worked out rather than measured — and the wrap
    // counts, because row zero's neighbour below is the last seeded row.
    let rusted = over(
        step(0.0, 1.0),
        MaterialGraph::builder("test:rusting")
            .node(
                "mask",
                instanced(
                    "weathering:rust",
                    &[("seed_mask", "height".into())],
                    &[("spread", SPREAD)],
                )
                .output(SurfaceOutput::Extra("rust_mask".to_owned())),
            )
            .output(PbrOutput::new().roughness("mask"))
            .into_graph(),
    );
    let mask = planes(&rusted, &BTreeMap::new()).roughness;
    let seeded = RESOLUTION / 2;
    for y in 0..RESOLUTION {
        let away = if y >= seeded {
            0
        } else {
            (seeded - y).min(y + 1)
        };
        // A texel of slack, for the reason `SPREAD_TEXELS` gives.
        if away <= SPREAD_TEXELS + 1 {
            continue;
        }
        for x in 0..RESOLUTION {
            let value = at(&mask, x, y);
            assert_eq!(
                value, 0.0,
                "rust {away} texels past the seed, which asked for {SPREAD_TEXELS}, at ({x}, {y})"
            );
        }
    }
    // Inside the front it is positive wherever the distance let it be, which is
    // everywhere inside: the bloom that breaks the front has a floor, so what
    // decides whether there is rust at all is the distance alone.
    let at_seed = at(&mask, 64, seeded);
    assert!(at_seed > 0.0, "no rust on the seed itself: {at_seed}");
    // And it reaches. Four fifths of the way out to `spread`, which is two
    // texels short of the front and well past the slack.
    let inside = seeded - SPREAD_TEXELS * 4 / 5;
    let near_front = at(&mask, 64, inside);
    assert!(
        near_front > 0.0,
        "the front stopped short of its spread: {near_front}"
    );
}

/// The curl the probe asks for, in UV and in whole texels, and the height it
/// hands in under the paint.
///
/// Three hundredths is seven and a half texels of band, counted in texels here
/// for the reason [`SPREAD_TEXELS`] gives. The height is the middle of the unit
/// so that a lip of a fifth cannot clip at the top, which would make "exceeds
/// its input" false for a reason that is not the compound's.
const CURL: f32 = 0.03;
const CURL_TEXELS: u32 = 7;
const UNDER_PAINT: f32 = 0.5;

#[test]
fn peeling_paint_lifts_the_height_in_the_lip_band_and_nowhere_else() {
    let torn = over(
        step(0.0, 1.0),
        MaterialGraph::builder("test:peeling")
            .node(
                "curled",
                instanced(
                    "weathering:peeling_paint",
                    &[
                        ("peel_mask", "height".into()),
                        ("height", UNDER_PAINT.into()),
                    ],
                    &[("lip", 0.2), ("curl_width", CURL)],
                )
                .output(SurfaceOutput::Height),
            )
            .output(PbrOutput::new().height("curled"))
            .into_graph(),
    );
    let height = planes(&torn, &BTreeMap::new())
        .height
        .expect("the probe binds a height");
    let bare = RESOLUTION / 2;
    for y in 0..RESOLUTION {
        let away = if y >= bare { 0 } else { (bare - y).min(y + 1) };
        for x in (0..RESOLUTION).step_by(37) {
            let value = at(&height, x, y);
            if y >= bare {
                // Inside the patch there is no film left to curl, and the
                // compound does not guess at a coat thickness either.
                assert_eq!(value, UNDER_PAINT, "the bare patch moved at ({x}, {y})");
            } else if away > CURL_TEXELS + 1 {
                assert_eq!(
                    value, UNDER_PAINT,
                    "sound paint {away} texels from the tear moved at ({x}, {y})"
                );
            } else if away + 1 < CURL_TEXELS {
                assert!(
                    value > UNDER_PAINT,
                    "the lip did not lift {away} texels from the tear at ({x}, {y}): {value}"
                );
            }
        }
    }
}

/// What a submerged texel's roughness has to come to, and how close is close
/// enough.
///
/// The compound's own `POOLED_ROUGHNESS`, written out because a test that read
/// the constant could not tell a change from a typo. The tolerance is there
/// because the value arrives through a `Mix`, and `a + (b - a) * 1` is `b` to
/// within a rounding rather than exactly.
const POOLED_ROUGHNESS: f32 = 0.02;
const ROUNDING: f32 = 1e-6;

/// Where the probe puts the waterline, and the two heights it straddles.
const WATERLINE: f32 = 0.5;
const POOL_FLOOR: f32 = 0.2;
const DRY_FACE: f32 = 0.8;

#[test]
fn moisture_floods_what_is_under_the_waterline_to_one_flat_wet_plane() {
    let rained_on = |output| {
        instanced(
            "weathering:moisture",
            &[("height", "height".into())],
            &[("level", WATERLINE), ("softness", 0.05)],
        )
        .output(output)
    };
    // A step from below the waterline to well above it: the bottom half of the
    // repeat is under water and the top half is only damp.
    let flooded = over(
        step(POOL_FLOOR, DRY_FACE),
        MaterialGraph::builder("test:rain")
            .node("rough", rained_on(SurfaceOutput::Roughness))
            .node("pooled", rained_on(SurfaceOutput::Height))
            .output(PbrOutput::new().roughness("rough").height("pooled"))
            .into_graph(),
    );
    let planes = planes(&flooded, &BTreeMap::new());
    let height = planes.height.expect("the probe binds a height");
    let submerged = RESOLUTION / 2;

    // Under the water: the water's own finish, and one flat plane at the
    // waterline whatever the surface below it was doing.
    let flat = at(&height, 0, 0);
    assert!(
        (flat - WATERLINE).abs() < ROUNDING,
        "the puddle did not stand at the waterline: {flat}"
    );
    for y in 0..submerged {
        for x in (0..RESOLUTION).step_by(37) {
            let rough = at(&planes.roughness, x, y);
            assert!(
                (rough - POOLED_ROUGHNESS).abs() < ROUNDING,
                "a submerged texel is not water-smooth at ({x}, {y}): {rough}"
            );
            assert_eq!(
                at(&height, x, y),
                flat,
                "the puddle is not flat at ({x}, {y})"
            );
        }
    }

    // Above it: damp rather than wet, so the finish is the substrate's less
    // what its pores took, and the surface is where the caller put it.
    let dry = at(&height, 0, submerged);
    assert!(
        dry > flat,
        "the face above the waterline sank into it: {dry} against {flat}"
    );
    for y in submerged..RESOLUTION {
        for x in (0..RESOLUTION).step_by(37) {
            let rough = at(&planes.roughness, x, y);
            assert!(
                rough > POOLED_ROUGHNESS + 0.1,
                "a damp texel reads as submerged at ({x}, {y}): {rough}"
            );
            assert_eq!(at(&height, x, y), dry, "the dry face moved at ({x}, {y})");
        }
    }
}

#[test]
fn a_live_wetness_through_moisture_stays_in_the_fragment_and_freezes_nothing() {
    // The idiom the whole library is built for: a wall baked into textures, one
    // uniform a game turns as the weather changes, and a compound between them.
    // It works because nothing in `weathering:moisture` is a buffered filter —
    // every other compound reads a neighbourhood of the height, and a live
    // parameter that reaches one of those is folded and named in the report.
    let library = stdlib::graphs();
    let rained_on = |output| {
        Subgraph::new("weathering:moisture")
            .input("height", "bedded")
            .input("wet", Input::param("rain"))
            .output(output)
    };
    let material = MaterialGraph::builder("test:rained-on")
        .param(Param::float("rain", 0.0).range(0.0, 1.0).live())
        .node("grain", Noise::value().period(8))
        .node("bedded", Levels::new("grain").out_range(0.25, 0.75))
        .node("albedo", rained_on(SurfaceOutput::BaseColor))
        .node("rough", rained_on(SurfaceOutput::Roughness))
        .node("metal", rained_on(SurfaceOutput::Metallic))
        .node("form", rained_on(SurfaceOutput::Height))
        .output(
            PbrOutput::new()
                .base_color("albedo")
                .roughness("rough")
                .metallic("metal")
                .height("form")
                .normal_strength(0.01),
        )
        .build_in(&library)
        .unwrap();
    let split = partition(
        &material,
        &Target::Shader {
            live: vec!["rain".to_owned()],
        },
        RESOLUTION,
    )
    .unwrap();
    assert_eq!(
        split.frozen(),
        &[] as &[String],
        "a pointwise compound froze a parameter: {}",
        split.report()
    );
    // The number is the claim. Sixty-seven instructions over the whole
    // fragment, and the shape of them is worth reading: the four channels come
    // to twenty-six between them — eighteen for the absorption into the colour,
    // eleven for the two roughness passes, seven for the flooded height, six
    // for the metalness, sharing the submergence they all read — and the other
    // forty-one are the derived normal, which is four taps of a height that is
    // live and so cannot be a plane. A caller who wants the cheap version binds
    // no height and keeps their own.
    assert_eq!(
        split.report().ops,
        67,
        "the fragment is not a handful of ops any more: {}",
        split.report()
    );
}

/// One pattern's exported field, read into roughness the way [`probe`] reads a
/// compound's mask.
///
/// A pattern takes no substrate, so there is nothing to bind: what comes back
/// is the field the graph was built to draw, at the parameters the test asked
/// for.
fn drawn(key: &str, extra: &str, params: &[(&str, f32)]) -> MaterialGraph {
    MaterialGraph::builder("test:pattern")
        .node(
            "field",
            instanced(key, &[], params).output(SurfaceOutput::Extra(extra.to_owned())),
        )
        .output(PbrOutput::new().roughness("field"))
        .into_graph()
}

/// The eight spokes a gradient is walked along, as a step in whole texels.
///
/// Whole texels rather than angles, because a spoke walked in texel space has a
/// radius that rises strictly at every step, while one walked in angles and
/// rounded to the nearest texel does not: two samples a fraction of a texel
/// apart can land the wrong way round, and a monotone test that failed on that
/// would be a test of the sampling rather than of the field.
const SPOKES: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

#[test]
fn a_radial_gradient_climbs_away_from_the_centre_and_never_turns_back() {
    let plane = planes(
        &drawn("patterns:radial_gradient", "gradient", &[]),
        &BTreeMap::new(),
    )
    .roughness;
    let centre = RESOLUTION / 2;
    for (dx, dy) in SPOKES {
        let mut last = at(&plane, centre, centre);
        // The middle of the repeat reads as the middle: a spoke that started
        // anywhere else would be a window centred on the wrong texel.
        assert!(last < 0.02, "the centre of the disc reads {last}");
        let mut climbed = last;
        for step in 1..i32::try_from(RESOLUTION / 2).unwrap() {
            let x = u32::try_from(i32::try_from(centre).unwrap() + dx * step).unwrap();
            let y = u32::try_from(i32::try_from(centre).unwrap() + dy * step).unwrap();
            let value = at(&plane, x, y);
            assert!(
                value >= last,
                "the gradient turned back {step} texels along ({dx}, {dy}): {value} after {last}"
            );
            last = value;
            climbed = climbed.max(value);
        }
        // And it arrives. A diagonal leaves the window before the edge of the
        // repeat and carries the fill, which is the one the rim ended on; a
        // spoke along an axis stops at the last texel *inside* the repeat,
        // whose centre is a texel and a half short of the rim, so it reads a
        // percent or so under one. Both are the top of the ramp.
        assert!(
            climbed > 0.98,
            "the gradient never arrived along ({dx}, {dy}): {climbed}"
        );
    }
}

/// How many places round the ring the sweep is read at, and how far out.
///
/// Forty-eight samples at a radius of three tenths are nearly five texels apart
/// at [`RESOLUTION`], which is far enough that rounding each to a texel cannot
/// put two of them the wrong way round. The ring is well inside the window, and
/// the walk starts and ends clear of the seam the sweep wraps at.
const RING_SAMPLES: u32 = 48;
const RING_RADIUS: f32 = 0.3;
const CLEAR_OF_THE_SEAM: f32 = 0.02;

#[test]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the ring is a point in UV read at the texel that holds it, and \
              the point is inside the repeat by construction: the conversion \
              is the floor of a number between zero and the resolution"
)]
fn an_angular_gradient_sweeps_once_round_a_ring_without_turning_back() {
    let plane = planes(
        &drawn("patterns:angular_gradient", "gradient", &[]),
        &BTreeMap::new(),
    )
    .roughness;
    let span = 1.0 - 2.0 * CLEAR_OF_THE_SEAM;
    let mut first = None;
    let mut last = 0.0;
    for sample in 0..RING_SAMPLES {
        let turns = CLEAR_OF_THE_SEAM
            + span * f32::from(u16::try_from(sample).unwrap())
                / f32::from(u16::try_from(RING_SAMPLES).unwrap());
        let angle = turns * std::f32::consts::TAU;
        let texel = |offset: f32| {
            let uv = 0.5 + RING_RADIUS * offset;
            (uv * f32::from(u16::try_from(RESOLUTION).unwrap())) as u32
        };
        let value = at(&plane, texel(angle.cos()), texel(angle.sin()));
        if let Some(first) = first {
            assert!(
                value > last,
                "the sweep turned back at {turns} turns: {value} after {last}"
            );
            let _ = first;
        } else {
            first = Some(value);
        }
        last = value;
    }
    let first = first.expect("the ring was sampled");
    // One turn and no more: the sweep starts where it says it does and arrives
    // at one, rather than going round twice or stopping halfway.
    assert!(first < 0.05, "the sweep did not start at its seam: {first}");
    assert!(last > 0.95, "the sweep did not finish a turn: {last}");
}

/// The rings [`patterns:wood`](ashlar_material::stdlib::wood) lays across the
/// repeat.
///
/// Written out rather than read off the module, so that a change to the count
/// and a typo in it are two different test failures.
const WOOD_RINGS: usize = 24;

#[test]
fn wood_lays_one_growth_ring_per_ring_count_down_the_board() {
    // Nothing displacing the rings: the coarse bend and the fibre both push the
    // ring lines about, and a field pushed about by a noise has as many local
    // maxima as the noise does. What is left is the wave itself, which is what
    // the ring count is a claim about.
    let plane = planes(
        &drawn("patterns:wood", "rings", &[("warp", 0.0), ("fibre", 0.0)]),
        &BTreeMap::new(),
    )
    .roughness;
    // One column, because with nothing warping it the field is the same down
    // every one of them.
    let column: Vec<f32> = (0..RESOLUTION).map(|y| at(&plane, 64, y)).collect();
    let rows = column.len();
    // Greater than the row below and at least the row above, wrapped: the rule
    // counts exactly one maximum per crest, including the crests the sampling
    // straddles evenly and which therefore come back as two equal rows.
    let crests = (0..rows)
        .filter(|&y| {
            let below = column[(y + rows - 1) % rows];
            let above = column[(y + 1) % rows];
            column[y] > below && column[y] >= above
        })
        .count();
    assert_eq!(
        crests, WOOD_RINGS,
        "the board has {crests} growth rings down it rather than {WOOD_RINGS}"
    );
}

/// The lattice [`patterns:cracks`](ashlar_material::stdlib::cracks) breaks on,
/// written out for the reason [`WOOD_RINGS`] is.
const CRACK_CELLS: u32 = 32;
const CRACK_SEED: u32 = 47;

/// What the probe asks the cracks for, and how far from a cell boundary the
/// surface is still allowed to have moved.
///
/// The crack is `width` wide and the ground slumps for `chamfer` either side of
/// it, both in UV, and the boundary field answers in cell units — so the
/// furthest a moved texel can be from a boundary is `(width + chamfer)` times
/// the cell count, plus the shoulder ramp the threshold puts on the line, plus
/// a texel for the jump flood's own quantisation. Over [`CRACK_CELLS`] cells
/// that is `0.012 * 32 + 0.15 + 32 / 256`, a little over two thirds of a cell,
/// and the bound below is three quarters of one.
const CRACK_WIDTH: f32 = 0.004;
const CRACK_CHAMFER: f32 = 0.008;
const OFF_THE_LINE: f32 = 0.75;

#[test]
fn cracks_cut_the_height_on_the_cell_boundaries_and_nowhere_else() {
    let crazed = MaterialGraph::builder("test:crazing")
        // The same lattice the graph breaks on, so that the test knows where
        // every line is rather than measuring where they ended up.
        .node(
            "plates",
            Voronoi::new()
                .period(CRACK_CELLS)
                .seed(CRACK_SEED)
                .output(VoronoiOutput::Edge),
        )
        .node(
            "cracked",
            instanced(
                "patterns:cracks",
                &[],
                &[
                    // Nothing wandering: a crack that had been warped off its
                    // cell would be somewhere this test cannot work out.
                    ("warp", 0.0),
                    ("width", CRACK_WIDTH),
                    ("chamfer", CRACK_CHAMFER),
                ],
            )
            .output(SurfaceOutput::Height),
        )
        .output(PbrOutput::new().roughness("plates").height("cracked"))
        .into_graph();
    let planes = planes(&crazed, &BTreeMap::new());
    let height = planes.height.expect("the probe binds a height");
    let boundary = planes.roughness;

    let face = height.iter().copied().fold(f32::MIN, f32::max);
    let floor = height.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        floor < face,
        "the surface is flat: nothing was cracked at all"
    );
    for (index, value) in height.iter().enumerate() {
        if *value < face {
            let edge = boundary[index];
            assert!(
                edge < OFF_THE_LINE,
                "the surface moved {edge} cells from the nearest boundary, at texel {index}"
            );
        }
    }
    // And the deepest of it is on a line rather than merely near one.
    let deepest = height
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .expect("a plane of texels")
        .0;
    assert!(
        boundary[deepest] < CRACK_WIDTH * f32::from(u16::try_from(CRACK_CELLS).unwrap()),
        "the deepest texel is {} cells off the boundary",
        boundary[deepest]
    );
}

#[test]
fn moss_off_preserves_substrate_and_growth_only_adds_height() {
    let graph = ashlar_material::stdlib::moss();
    let off = planes(
        &graph,
        &BTreeMap::from([("amount".into(), ParamValue::Float(0.0))]),
    );
    assert!(off.base_color.iter().all(|v| *v == [0.5; 3]));
    assert!(off.roughness.iter().all(|v| *v == 0.8));
    assert!(off.height.as_ref().unwrap().iter().all(|v| *v == 0.5));
    let grown = planes(
        &graph,
        &BTreeMap::from([("amount".into(), ParamValue::Float(0.8))]),
    );
    assert!(grown.height.as_ref().unwrap().iter().all(|v| *v >= 0.5));
    assert!(grown.height.as_ref().unwrap().iter().any(|v| *v > 0.51));
    let excluded = MaterialGraph::builder("test:excluded-moss")
        .layer(
            "moss",
            Subgraph::new("weathering:moss")
                .input("bias", 0.0)
                .param("amount", ParamValue::Float(1.0)),
            &[SurfaceOutput::BaseColor, SurfaceOutput::Height],
        )
        .output(
            PbrOutput::new()
                .base_color("moss.base_color")
                .height("moss.height"),
        )
        .into_graph();
    let excluded = planes(&excluded, &BTreeMap::new());
    assert_eq!(excluded.base_color, off.base_color);
    assert_eq!(excluded.height, off.height);
}

// The existing moss-carpet reference mixes a six-cell habitat with power-of-
// two fibres. Preserve its reviewed bytes during the namespace move, and
// constrain the inherited warning exception to these four known LCM joins.
fn assert_reference_warnings(key: &str, warnings: &[ashlar_material::GraphWarning]) {
    if key == "library:moss-carpet" {
        assert_eq!(warnings.len(), 4);
        for warning in warnings {
            assert!(warning.path.contains("weathering:moss@"));
            assert!(
                warning.message.starts_with("period 192x192 is larger")
                    || warning.message.starts_with("period 768x768 is larger")
            );
        }
    } else if key.starts_with("library:soi-cobblestone") {
        assert_eq!(warnings.len(), 19, "{key}: {warnings:?}");
        assert!(warnings.iter().all(|w| w.message.starts_with("period ")
            && w.message.contains("is larger than every input")));
    } else {
        assert!(warnings.is_empty(), "{key}: {warnings:?}");
    }
}
