//! What the buffered filters compute, and that every one of them wraps.
//!
//! A buffered filter is the one place this crate reads a neighbourhood, and so
//! the one place a seam can appear that period inference would not catch: the
//! period of a blurred noise is right whether or not the blur read across the
//! wrap, and only the picture is wrong. Every filter here is therefore checked
//! twice — once for what it computes, against a number worked out by hand, and
//! once for what it does at the seam, against a field whose answer is different
//! if the filter clamped instead of wrapping.
//!
//! The fields are step functions of the coordinate rather than noises, because
//! a claim like "the distance at four texels along is four texels" has to be
//! about something whose answer is known.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::float_cmp,
    reason = "the occlusion of a flat field is exactly one and an opening of a \
              square is exactly that square; a margin would pass a filter that \
              had quietly stopped being either"
)]
#![allow(
    clippy::cast_precision_loss,
    reason = "a texel index is bounded by the resolution these tests bake at"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    Channel, MaterialGraph, MaterialGraphBuilder, MaterialGraphLibrary, MathOp, PbrOutput,
    bake::{BakeRequest, Planes, rasterise, rasterise_with},
    ir::{Target, lower},
    nodes::{
        Blur, Buffer, Curvature, CurvatureKind, Decompose, Dilate, Distance, EdgeDetect, Erode,
        Invert, Math, Noise, OcclusionFromHeight, Uv,
    },
    planes::{BakeCache, PlaneKey, plane_closure, plane_keys, rasterise_buffers, rasterise_wanted},
};

/// The resolution every case here bakes at: the smallest a bake allows, which
/// keeps a jump flood and an eight-direction horizon march inside a test's
/// worth of time.
const N: u32 = 256;

/// Three threads rather than the default. A test binary runs its cases in
/// parallel already, and naming a count is what makes the division of the rows
/// the same on every machine.
const THREADS: usize = 3;

fn request<'a>(
    graph: &'a MaterialGraph,
    library: &'a MaterialGraphLibrary,
    params: &'a BTreeMap<String, ashlar_material::ParamValue>,
    resolution: u32,
) -> BakeRequest<'a> {
    BakeRequest {
        graph,
        library,
        params,
        resolution,
        mips: false,
        threads: NonZeroUsize::new(THREADS),
    }
}

/// A builder with the texel's two axes already decomposed as `u` and `v`.
fn wired() -> MaterialGraphBuilder {
    MaterialGraph::builder("test:buffered")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
}

/// The `f32` planes of a graph, before anything is quantised.
fn planes(graph: &MaterialGraph, resolution: u32) -> Planes {
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    rasterise(&request(graph, &library, &params, resolution))
        .unwrap()
        .0
}

/// How many planes a graph's buffered filters needed.
fn buffers(graph: &MaterialGraph) -> usize {
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    rasterise(&request(graph, &library, &params, N))
        .unwrap()
        .1
        .buffers
}

/// One texel of a square plane.
fn at(plane: &[f32], x: u32, y: u32) -> f32 {
    plane[(y * N + x) as usize]
}

/// The texel a UV lands in, for a claim written in UV rather than in texels.
///
/// The cases below are authored as distances across the repeat — a band at a
/// half, a smear of a twentieth — because that is what a radius is, so reading
/// one back wants the texel that coordinate names.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "every coordinate here is inside the unit, so the product is inside the resolution"
)]
fn texel_of(u: f32) -> u32 {
    (u * N as f32) as u32
}

#[test]
fn a_blur_of_a_constant_is_that_constant() {
    // The kernel is normalised over the taps it keeps, so a field with nothing
    // to smear comes back untouched. A blur that also darkened by a fraction of
    // a percent would be invisible in a picture and wrong in every mask that
    // went through it.
    for radius in [0.001_f32, 0.01, 0.05, 0.2] {
        let graph = wired()
            .node("n", Blur::new(0.25).radius(radius))
            .output(PbrOutput::new().roughness("n"))
            .into_graph();
        let roughness = planes(&graph, N).roughness;
        for texel in &roughness {
            assert!(
                (texel - 0.25).abs() < 1e-6,
                "a blur of radius {radius} moved a constant to {texel}"
            );
        }
    }
}

#[test]
fn a_blur_reads_across_the_seam_rather_than_stopping_at_it() {
    // One bright column at the origin. A wrapped blur spreads it both ways, so
    // the last column of the plane — the neighbour the other side of the seam —
    // is as bright as the second column. A blur that clamped at the edge would
    // leave it at zero, and a wall built from the texture would show the join.
    let graph = wired()
        .node("edge", Math::new(MathOp::Step, "u", 1.0 / N as f32))
        .node("column", Invert::new("edge"))
        .node("n", Blur::new("column").radius(3.0 / N as f32))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let roughness = planes(&graph, N).roughness;

    assert!(at(&roughness, 0, 0) > 0.0);
    assert!(
        at(&roughness, N - 1, 0) > 0.05,
        "the column across the seam is {}",
        at(&roughness, N - 1, 0)
    );
    assert!(
        (at(&roughness, N - 1, 0) - at(&roughness, 1, 0)).abs() < 1e-6,
        "a wrapped blur is symmetric about the column it blurred: {} and {}",
        at(&roughness, N - 1, 0),
        at(&roughness, 1, 0),
    );
    // And nothing was lost: a separable normalised kernel moves the column's
    // weight about without adding to it or taking from it.
    let total: f32 = roughness.iter().sum();
    assert!((total - N as f32).abs() < 0.05, "{total}");
}

#[test]
fn a_flat_height_is_open_everywhere() {
    // Nothing is above the horizon of a flat field, in any direction, at any
    // radius: the occlusion is one exactly, and a filter that drifted to 0.998
    // would be a texture set that is slightly dirty everywhere for no reason.
    let graph = wired()
        .node("n", OcclusionFromHeight::new(0.5).radius(0.05))
        .output(PbrOutput::new().occlusion("n").height(0.5))
        .into_graph();
    for texel in &planes(&graph, N).occlusion {
        assert_eq!(*texel, 1.0);
    }
}

/// A square pit of half-width 0.02 in a field that is one everywhere else.
///
/// Narrower than the radius the occlusion is read at, so its floor sees the rim
/// above it in every direction.
fn pit() -> MaterialGraph {
    wired()
        .node(
            "du",
            Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", 0.5)),
        )
        .node(
            "dv",
            Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "v", 0.5)),
        )
        .node("outside_u", Math::new(MathOp::Step, "du", 0.02))
        .node("outside_v", Math::new(MathOp::Step, "dv", 0.02))
        .node("height", Math::new(MathOp::Max, "outside_u", "outside_v"))
        .node("ao", OcclusionFromHeight::new("height").radius(0.05))
        .output(
            PbrOutput::new()
                .occlusion("ao")
                .height("height")
                .normal_strength(0.01),
        )
        .into_graph()
}

#[test]
fn a_pit_darkens_its_own_floor_and_nothing_else() {
    let planes = planes(&pit(), N);
    let (occlusion, height) = (&planes.occlusion, planes.height.as_ref().unwrap());

    // The floor is deep and narrow, so most of its sky is gone.
    let floor = at(occlusion, N / 2, N / 2);
    assert!(floor < 0.5, "the floor of the pit is {floor}");

    // And the ground is open: a texel outside the pit has nothing above it, the
    // pit included, because a pit is a depression. This is the claim that
    // separates a horizon filter from a blur of the height.
    assert_eq!(at(occlusion, N / 4, N / 4), 1.0);
    for (index, texel) in occlusion.iter().enumerate() {
        assert!(
            *texel == 1.0 || height[index] < 1.0,
            "texel {index} is occluded at {texel} while standing on the ground",
        );
    }
}

#[test]
fn an_occlusion_marches_across_the_seam() {
    // The same pit, moved to the corner so that half of it is on each side of
    // both seams. Its floor is as dark as the middle of the plane was, which it
    // can only be if the march wrapped: on a clamped read the corner texel
    // would see a rim on two sides and open sky on the other two.
    let graph = wired()
        .node(
            "du",
            Math::unary(
                MathOp::Abs,
                Math::new(
                    MathOp::Sub,
                    Math::unary(MathOp::Fract, Math::new(MathOp::Add, "u", 0.5)),
                    0.5,
                ),
            ),
        )
        .node(
            "dv",
            Math::unary(
                MathOp::Abs,
                Math::new(
                    MathOp::Sub,
                    Math::unary(MathOp::Fract, Math::new(MathOp::Add, "v", 0.5)),
                    0.5,
                ),
            ),
        )
        .node("outside_u", Math::new(MathOp::Step, "du", 0.02))
        .node("outside_v", Math::new(MathOp::Step, "dv", 0.02))
        .node("height", Math::new(MathOp::Max, "outside_u", "outside_v"))
        .node("ao", OcclusionFromHeight::new("height").radius(0.05))
        .output(
            PbrOutput::new()
                .occlusion("ao")
                .height("height")
                .normal_strength(0.01),
        )
        .into_graph();
    let corner = planes(&graph, N).occlusion;
    let middle = planes(&pit(), N).occlusion;
    assert!(
        (at(&corner, 0, 0) - at(&middle, N / 2, N / 2)).abs() < 1e-3,
        "the corner of the plane is {} where the middle was {}",
        at(&corner, 0, 0),
        at(&middle, N / 2, N / 2),
    );
}

/// A mask that is one in the texel at the origin and zero everywhere else, read
/// through a distance whose range is one so that the output is the distance
/// itself in UV.
fn spot() -> MaterialGraph {
    wired()
        .node("edge_u", Math::new(MathOp::Step, "u", 1.0 / N as f32))
        .node("edge_v", Math::new(MathOp::Step, "v", 1.0 / N as f32))
        .node(
            "spot",
            Math::new(MathOp::Mul, Invert::new("edge_u"), Invert::new("edge_v")),
        )
        .node("n", Distance::new("spot").threshold(0.5).range(1.0))
        .output(PbrOutput::new().roughness("n"))
        .into_graph()
}

#[test]
fn a_distance_from_one_texel_grows_a_texel_at_a_time() {
    let roughness = planes(&spot(), N).roughness;
    assert_eq!(
        at(&roughness, 0, 0),
        0.0,
        "the mask itself is at no distance"
    );
    for step in [1_u32, 2, 4, 8, 31, 64] {
        let along = at(&roughness, step, 0);
        assert!(
            (along - step as f32 / N as f32).abs() < 1e-6,
            "{step} texels along is {along}, not {}",
            step as f32 / N as f32,
        );
        // And the diagonal is the diagonal, which is what makes it a Euclidean
        // distance rather than a count of steps.
        let diagonal = at(&roughness, step, step);
        let expected = (2.0_f32).sqrt() * step as f32 / N as f32;
        assert!(
            (diagonal - expected).abs() < 1e-6,
            "{step} texels along both axes is {diagonal}, not {expected}",
        );
    }
}

#[test]
fn a_distance_measures_the_short_way_round_the_wrap() {
    let roughness = planes(&spot(), N).roughness;
    // The last texel of the first row is one texel from the mask, not 255.
    assert!((at(&roughness, N - 1, 0) - 1.0 / N as f32).abs() < 1e-6);
    assert!((at(&roughness, 0, N - 1) - 1.0 / N as f32).abs() < 1e-6);
    // The furthest texel is the opposite corner of the torus.
    let furthest = at(&roughness, N / 2, N / 2);
    let expected = (2.0_f32).sqrt() * 0.5;
    assert!((furthest - expected).abs() < 1e-6, "{furthest}");
}

#[test]
fn a_mask_with_nothing_in_it_is_far_from_everywhere() {
    let graph = wired()
        .node("n", Distance::new(0.0).threshold(0.5).range(1.0))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let expected = (2.0_f32).sqrt() * 0.5;
    for texel in &planes(&graph, N).roughness {
        assert!((texel - expected).abs() < 1e-6, "{texel}");
    }
}

/// A square of half-width 0.25, whose edges land on texel boundaries at this
/// resolution, as one, and zero outside it.
fn square(build: impl FnOnce(MaterialGraphBuilder) -> MaterialGraphBuilder) -> MaterialGraph {
    build(
        wired()
            .node(
                "du",
                Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", 0.5)),
            )
            .node(
                "dv",
                Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "v", 0.5)),
            )
            .node("outside_u", Math::new(MathOp::Step, "du", 0.25))
            .node("outside_v", Math::new(MathOp::Step, "dv", 0.25))
            .node(
                "square",
                Invert::new(Math::new(MathOp::Max, "outside_u", "outside_v")),
            ),
    )
    .into_graph()
}

#[test]
fn eroding_a_square_and_dilating_it_back_gives_the_square() {
    // The opening of a mask by a structuring element is the mask again wherever
    // the element fits inside it, and this element is the square neighbourhood
    // of the radius: a square mask larger than it comes back unchanged, corners
    // and all. A disc would round those corners, which is the trade the
    // separable square takes and the reason this test can be an equality.
    let mask = square(|builder| builder.output(PbrOutput::new().roughness("square")));
    let opened = square(|builder| {
        builder
            .node("shrunk", Erode::new("square").radius(0.02))
            .node("grown", Dilate::new("shrunk").radius(0.02))
            .output(PbrOutput::new().roughness("grown"))
    });
    assert_eq!(planes(&mask, N).roughness, planes(&opened, N).roughness);
}

#[test]
fn an_erosion_shrinks_a_mask_by_its_radius_and_a_dilation_grows_it() {
    let shrunk = square(|builder| {
        builder
            .node("n", Erode::new("square").radius(8.0 / N as f32))
            .output(PbrOutput::new().roughness("n"))
    });
    let grown = square(|builder| {
        builder
            .node("n", Dilate::new("square").radius(8.0 / N as f32))
            .output(PbrOutput::new().roughness("n"))
    });
    // The square runs from texel 64 to texel 191. Eroded by eight it starts at
    // 72; dilated by eight it starts at 56.
    let middle = N / 2;
    let shrunk = planes(&shrunk, N).roughness;
    let grown = planes(&grown, N).roughness;
    assert_eq!(at(&shrunk, 71, middle), 0.0);
    assert_eq!(at(&shrunk, 72, middle), 1.0);
    assert_eq!(at(&grown, 55, middle), 0.0);
    assert_eq!(at(&grown, 56, middle), 1.0);
}

#[test]
fn a_morphology_reaches_across_the_seam() {
    // A column of mask at the origin, dilated: it grows both ways, and the
    // texels it reaches the other way are the last ones of the row.
    let graph = wired()
        .node("edge", Math::new(MathOp::Step, "u", 1.0 / N as f32))
        .node("column", Invert::new("edge"))
        .node("n", Dilate::new("column").radius(4.0 / N as f32))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let roughness = planes(&graph, N).roughness;
    assert_eq!(at(&roughness, 4, 0), 1.0);
    assert_eq!(at(&roughness, 5, 0), 0.0);
    assert_eq!(at(&roughness, N - 4, 0), 1.0);
    assert_eq!(at(&roughness, N - 5, 0), 0.0);
}

#[test]
fn a_buffer_at_its_own_resolution_is_a_plane_of_that_size() {
    // A plane pinned at sixteen texels holds sixteen cells, and everything
    // between them is the bilinear read of [`Op::Sample`]. So along a row, the
    // bake's texels inside one of those cells are on a straight line: the
    // differences between neighbours are the same number. At the bake's own
    // resolution they are not, because the field underneath is a noise.
    let pinned = wired()
        .node("grain", Noise::value().period(8))
        .node("n", Buffer::new("grain").resolution(16))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let plain = wired()
        .node("grain", Noise::value().period(8))
        .output(PbrOutput::new().roughness("grain"))
        .into_graph();
    let steps = |plane: &[f32]| -> Vec<f32> {
        (1..7)
            .map(|x| at(plane, x, 0) - at(plane, x - 1, 0))
            .collect()
    };
    let pinned = steps(&planes(&pinned, N).roughness);
    let plain = steps(&planes(&plain, N).roughness);
    for step in &pinned {
        assert!(
            (step - pinned[0]).abs() < 1e-6,
            "a pinned plane is linear between its own texels: {pinned:?}",
        );
    }
    assert!(
        plain.iter().any(|step| (step - plain[0]).abs() > 1e-6),
        "the field itself is not, or this test is checking nothing: {plain:?}",
    );
}

#[test]
fn two_nodes_over_one_input_share_a_plane_and_a_dead_one_leaves_none() {
    let two = |a: f32, b: f32| {
        wired()
            .node("grain", Noise::value().period(8))
            .node("soft", Blur::new("grain").radius(a))
            .node("other", Blur::new("grain").radius(b))
            .node("n", Math::new(MathOp::Add, "soft", "other"))
            .output(PbrOutput::new().roughness("n"))
            .into_graph()
    };
    assert_eq!(
        buffers(&two(0.01, 0.01)),
        1,
        "the same filter over the same expression is one plane",
    );
    assert_eq!(buffers(&two(0.01, 0.02)), 2, "and two radii are two");

    let dead = wired()
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.01))
        .output(PbrOutput::new().roughness("grain"))
        .into_graph();
    assert_eq!(
        buffers(&dead),
        0,
        "a blur no output reads is not a plane anything pays for",
    );
}

#[test]
fn a_cache_handed_in_keeps_the_planes_and_answers_the_same_bytes() {
    let graph = wired()
        .node("grain", Noise::value().period(8))
        .node("n", Blur::new("grain").radius(0.02))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    let request = request(&graph, &library, &params, N);

    let mut cache = BakeCache::new();
    assert!(cache.is_empty());
    let (first, _) = rasterise_with(&request, &mut cache).unwrap();
    assert_eq!(cache.len(), 1);
    let (second, _) = rasterise_with(&request, &mut cache).unwrap();
    assert_eq!(cache.len(), 1, "the second bake found the plane it needed");
    assert_eq!(first.roughness, second.roughness);

    // And a cache is a cache of planes rather than of bakes: cleared, the same
    // request answers the same numbers again.
    cache.clear();
    let (third, _) = rasterise_with(&request, &mut cache).unwrap();
    assert_eq!(first.roughness, third.roughness);
}

/// A bake reads back what an earlier stage of it computed rather than
/// computing it again, and a stage that came out of the cache never ran — so
/// the same request must answer the same bytes with a cold cache, with a warm
/// one, and with a cache that holds some of its planes and not the rest.
#[test]
fn a_bake_whose_planes_came_out_of_the_cache_answers_the_bytes_it_rasterised() {
    let shared = wired()
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .output(PbrOutput::new().roughness("soft"))
        .into_graph();
    // The same blur, and a second plane over it: the first plane's key is the
    // key `shared` made, so a cache warmed by `shared` answers it and the
    // stage that would have kept its values for the outputs does not run.
    let deeper = wired()
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .node("edge", EdgeDetect::new("soft"))
        .output(
            PbrOutput::new()
                .roughness("soft")
                .height("edge")
                .base_color("grain"),
        )
        .into_graph();
    let (library, params) = (MaterialGraphLibrary::default(), BTreeMap::new());
    let deeper = request(&deeper, &library, &params, N);
    let shared = request(&shared, &library, &params, N);

    let mut cold = BakeCache::new();
    let (rasterised, _) = rasterise_with(&deeper, &mut cold).unwrap();

    let mut partial = BakeCache::new();
    rasterise_with(&shared, &mut partial).unwrap();
    assert_eq!(partial.len(), 1, "the blur, and not the edge over it");
    let (half_cached, _) = rasterise_with(&deeper, &mut partial).unwrap();
    assert_eq!(half_cached, rasterised, "one stage skipped, the same bytes");

    let (all_cached, _) = rasterise_with(&deeper, &mut partial).unwrap();
    assert_eq!(
        all_cached, rasterised,
        "every stage skipped, the same bytes"
    );
}

#[test]
fn rasterising_some_planes_answers_exactly_what_rasterising_all_of_them_did() {
    // The guarantee a caller holding half the answer already needs: asking for
    // one plane must give that plane, byte for byte, and must not skip
    // anything it reads. `smeared` is a blur of a blur, so wanting it wants
    // `soft` with it; `other` is nothing's dependency and must not be paid for.
    let graph = wired()
        .node("grain", Noise::value().period(8))
        .node("soft", Blur::new("grain").radius(0.02))
        .node("smeared", Blur::new("soft").radius(0.01))
        .node("other", Blur::new("grain").radius(0.05))
        .node("n", Math::new(MathOp::Add, "smeared", "other"))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let material = graph.build().unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    assert_eq!(ir.buffers().len(), 3, "soft, smeared and other");

    let all = rasterise_buffers(&ir, N, NonZeroUsize::new(THREADS), &mut BakeCache::new()).unwrap();
    // The blur of a blur, whichever plan the lowering made it: it is the one
    // of the three that needs another, and the sizes say so without this test
    // knowing the order plans come out in.
    let one = |index: usize| {
        (0..ir.buffers().len())
            .map(|at| at == index)
            .collect::<Vec<_>>()
    };
    let sizes: Vec<usize> = (0..ir.buffers().len())
        .map(|index| {
            plane_closure(&ir, &one(index))
                .iter()
                .filter(|needed| **needed)
                .count()
        })
        .collect();
    assert_eq!(
        sizes.iter().filter(|size| **size == 2).count(),
        1,
        "{sizes:?}"
    );
    let wanted = one(sizes.iter().position(|size| *size == 2).unwrap());

    let mut cache = BakeCache::new();
    let some = rasterise_wanted(&ir, N, &wanted, NonZeroUsize::new(THREADS), &mut cache).unwrap();
    let closure = plane_closure(&ir, &wanted);
    assert_eq!(cache.len(), 2, "the blur it reads, and no more");

    for key in plane_keys(&ir, N) {
        assert!(!key.is_empty());
    }
    for (index, (all, some)) in all.iter().zip(&some).enumerate() {
        match some {
            Some(plane) => assert_eq!(plane.lanes(), all.lanes(), "plane {index} moved"),
            None => assert!(!closure[index], "plane {index} was needed and skipped"),
        }
    }
}

/// How many blurred levels the key-growth case stacks. Small enough to stay a
/// test's worth of lowering and large enough that a key which doubled at every
/// level would be a thousand times the first one rather than ten.
const LEVELS: usize = 10;

#[test]
fn a_plane_key_is_the_sum_of_its_closure_and_not_the_product_of_it() {
    // The shape that decides whether `PlaneKey` is affordable on real content.
    // Each level blurs the sum of the two below it, so every plane's closure
    // holds every plane under it and every plane under it is reachable by more
    // than one path. A key that spelled a dependency out once per path — which
    // is how this key was first written — is Fibonacci in the number of levels:
    // at ten it is already a hundred times the first key, and `study:brick`'s
    // largest was 190 KiB where it is now 117. A key that writes its closure as
    // a list is linear, and the bound below is what says which one this is.
    let mut builder = wired()
        .node("grain", Noise::value().period(8))
        .node("b0", Blur::new("grain").radius(0.020))
        .node("b1", Blur::new("grain").radius(0.021));
    for level in 2..LEVELS {
        let sum = format!("m{level}");
        builder = builder
            .node(
                sum.clone(),
                Math::new(
                    MathOp::Add,
                    format!("b{}", level - 1),
                    format!("b{}", level - 2),
                ),
            )
            .node(
                format!("b{level}"),
                Blur::new(sum).radius(0.02 + level as f32 * 0.001),
            );
    }
    let graph = builder
        .output(PbrOutput::new().roughness(format!("b{}", LEVELS - 1)))
        .into_graph();
    let material = graph.build().unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    assert_eq!(ir.buffers().len(), LEVELS, "one plan per blurred level");

    let keys = plane_keys(&ir, N);
    let first = keys.first().unwrap().len();
    let largest = keys.iter().map(PlaneKey::len).max().unwrap();
    assert!(first > 0);
    // Linear with room to spare: a level adds one plan's own encoding, and no
    // plan is written twice however many paths reach it. The nested encoding
    // this replaced fails this by two orders of magnitude.
    assert!(
        largest < 2 * LEVELS * first,
        "the deepest of {LEVELS} keys is {largest} bytes against a first of {first}"
    );
    // And the difference between consecutive levels is a plan, not a doubling.
    for pair in keys.windows(2) {
        let (below, above) = (pair[0].len(), pair[1].len());
        assert!(
            above <= below + 4 * first,
            "a level took a key from {below} to {above} bytes"
        );
    }
}

#[test]
fn a_directional_blur_smears_along_its_angle_and_leaves_the_other_axis_alone() {
    // One bright column at u = 0. Blurred along u it spreads across the plane;
    // blurred along v — the direction the column already runs — every texel of
    // it reads its own value, so the column comes back exactly as it was. That
    // pair is the whole claim of the node: a directional blur is anisotropic,
    // and which axis it leaves sharp is what makes a brushed metal.
    let column = |angle: f32| {
        let graph = wired()
            .node("edge", Math::new(MathOp::Step, "u", 1.0 / N as f32))
            .node("column", Invert::new("edge"))
            .node(
                "n",
                Blur::directional("column", angle).radius(6.0 / N as f32),
            )
            .output(PbrOutput::new().roughness("n"))
            .into_graph();
        planes(&graph, N).roughness
    };

    let across = column(0.0);
    assert!(at(&across, 0, 0) < 0.35, "{}", at(&across, 0, 0));
    assert!(at(&across, 3, 0) > 0.05, "{}", at(&across, 3, 0));

    let along = column(90.0);
    for row in [0, 1, N / 2, N - 1] {
        assert!(
            (at(&along, 0, row) - 1.0).abs() < 1e-5,
            "a blur along the column moved it: {}",
            at(&along, 0, row)
        );
        assert!(at(&along, 3, row) < 1e-5, "{}", at(&along, 3, row));
    }
}

#[test]
fn a_directional_blur_reads_across_the_seam() {
    // The same column as the Gaussian's seam case, and the same claim: the
    // texels the other side of the wrap are as bright as the ones this side,
    // so the wall it is baked into has no line down it.
    let graph = wired()
        .node("edge", Math::new(MathOp::Step, "u", 1.0 / N as f32))
        .node("column", Invert::new("edge"))
        .node("n", Blur::directional("column", 0.0).radius(3.0 / N as f32))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let roughness = planes(&graph, N).roughness;
    assert!(
        at(&roughness, N - 1, 0) > 0.05,
        "the column across the seam is {}",
        at(&roughness, N - 1, 0)
    );
    assert!(
        (at(&roughness, N - 1, 0) - at(&roughness, 1, 0)).abs() < 1e-5,
        "{} and {}",
        at(&roughness, N - 1, 0),
        at(&roughness, 1, 0),
    );
}

#[test]
fn a_slope_blur_leaves_flat_ground_exactly_as_it_found_it() {
    // Nothing is downhill of anywhere on a level field, so every one of the
    // walk's steps stands still and the mean of the samples is the sample. A
    // filter that moved by the raw gradient rather than by a normalised one
    // would also pass this; what it would not pass is the case below, where the
    // reach has to be the radius rather than the radius times the slope.
    let graph = wired()
        .node("grain", Noise::value().period(8))
        .node("n", Blur::slope("grain", 0.5).radius(0.05))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let smeared = planes(&graph, N).roughness;
    let plain = planes(
        &wired()
            .node("grain", Noise::value().period(8))
            .output(PbrOutput::new().roughness("grain"))
            .into_graph(),
        N,
    )
    .roughness;
    let worst = smeared
        .iter()
        .zip(&plain)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max);
    // Not bit for bit, and the reason is arithmetic rather than the walk: the
    // answer is the mean of `steps + 1` samples, and the mean of seventeen
    // copies of a number is that number to within the last place of an `f32`.
    assert!(
        worst < 1e-6,
        "a slope blur over a level field moved by {worst}"
    );
}

#[test]
fn a_slope_blur_carries_a_band_up_the_hill_it_walks_down() {
    // A height that rises with u, and a bright band across the middle of it. A
    // texel reads the source from `radius` down the slope, which is toward
    // smaller u, so the band is carried toward *larger* u — up the hill — and
    // the ground below it is left dark. The smear reaches the radius and no
    // further, which is the claim the normalised step makes.
    let band = 0.5;
    let radius = 0.05;
    let graph = wired()
        .node("height", Math::new(MathOp::Mul, "u", 1.0))
        .node(
            "from_band",
            Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", band)),
        )
        .node("source", Math::new(MathOp::Step, "from_band", 0.004))
        .node("mask", Invert::new("source"))
        .node("n", Blur::slope("mask", "height").radius(radius).steps(16))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let smeared = planes(&graph, N).roughness;
    let column = |u: f32| at(&smeared, texel_of(u), N / 2);

    assert!(column(band) > 0.05, "the band itself is {}", column(band));
    let uphill = column(band + radius * 0.5);
    let downhill = column(band - radius * 0.5);
    assert!(
        uphill > 0.05,
        "the band was not carried up the hill: {uphill}"
    );
    assert!(
        downhill < uphill * 0.25,
        "the band ran downhill as well: {downhill} against {uphill}"
    );
    // And it stops at the radius: a step of `radius / steps` taken `steps`
    // times reaches exactly `radius`, whatever the height's own scale.
    assert!(
        column(band + radius * 1.6) < 1e-4,
        "the smear ran past its radius: {}",
        column(band + radius * 1.6)
    );
}

#[test]
fn a_slope_blur_rasterises_its_height_as_a_plane_of_its_own() {
    // Two planes: the field being smeared, and the height being walked down.
    // The second is the reason a filter may name another plane at all, and a
    // lowering that dropped it — nothing samples it — would leave the filter
    // walking on zeros.
    let graph = wired()
        .node("grain", Noise::value().period(8))
        .node("slope", Noise::value().period(4).seed(2))
        .node("n", Blur::slope("grain", "slope").radius(0.04))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    assert_eq!(buffers(&graph), 2);

    // And the walk really reads it: the same source over a different height is
    // a different picture.
    let other = wired()
        .node("grain", Noise::value().period(8))
        .node("slope", Noise::value().period(4).seed(3))
        .node("n", Blur::slope("grain", "slope").radius(0.04))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    assert_ne!(planes(&graph, N).roughness, planes(&other, N).roughness);
}

#[test]
fn a_curvature_is_positive_on_a_crest_and_negative_in_a_hollow() {
    // A single groove across the field, and a single ridge beside it. What the
    // filter answers is how far the height stands above its own neighbourhood,
    // so the ridge is positive, the groove negative, and the flat ground
    // between them zero — a ramp would be zero as well, which is what tells a
    // curvature from an edge.
    for kind in [CurvatureKind::Laplacian, CurvatureKind::Blurred] {
        let graph = wired()
            .node(
                "from_ridge",
                Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", 0.25)),
            )
            .node(
                "ridge",
                Invert::new(Math::new(MathOp::Step, "from_ridge", 0.02)),
            )
            .node(
                "from_groove",
                Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", 0.75)),
            )
            .node(
                "groove",
                Invert::new(Math::new(MathOp::Step, "from_groove", 0.02)),
            )
            .node("relief", Math::new(MathOp::Mul, "groove", 0.5))
            .node(
                "bed",
                Math::new(MathOp::Add, 0.5, Math::new(MathOp::Mul, "ridge", 0.3)),
            )
            .node("height", Math::new(MathOp::Sub, "bed", "relief"))
            .node(
                "n",
                Curvature::new("height")
                    .radius(0.05)
                    .kind(kind)
                    .strength(1.0),
            )
            .output(PbrOutput::new().roughness("n").height("height"))
            .into_graph();
        let curvature = planes(&graph, N).roughness;
        let column = |u: f32| at(&curvature, texel_of(u), N / 2);
        assert!(column(0.25) > 0.55, "{kind:?}: ridge is {}", column(0.25));
        assert!(column(0.75) < 0.45, "{kind:?}: groove is {}", column(0.75));
        assert!(
            (column(0.5) - 0.5).abs() < 1e-3,
            "{kind:?}: flat ground is {}",
            column(0.5)
        );
    }
}

#[test]
fn the_two_sides_of_a_curvature_are_the_masks_a_worn_surface_wants() {
    // Peaks and cavity are one filter read twice, so they cost one plane, and
    // each is the side of it the other is not.
    let graph = wired()
        .node(
            "from_ridge",
            Math::unary(MathOp::Abs, Math::new(MathOp::Sub, "u", 0.25)),
        )
        .node(
            "ridge",
            Invert::new(Math::new(MathOp::Step, "from_ridge", 0.02)),
        )
        .node(
            "height",
            Math::new(MathOp::Add, 0.4, Math::new(MathOp::Mul, "ridge", 0.3)),
        )
        .node(
            "peaks",
            Curvature::peaks("height").radius(0.05).strength(4.0),
        )
        .node(
            "cavity",
            Curvature::cavity("height").radius(0.05).strength(4.0),
        )
        .output(
            PbrOutput::new()
                .roughness("peaks")
                .metallic("cavity")
                .height("height"),
        )
        .into_graph();
    assert_eq!(buffers(&graph), 1, "one plane read from both sides");

    let planes = planes(&graph, N);
    let (peaks, cavity) = (&planes.roughness, &planes.metallic);
    let column = |plane: &[f32], u: f32| at(plane, texel_of(u), N / 2);
    assert!(column(peaks, 0.25) > 0.5, "{}", column(peaks, 0.25));
    assert_eq!(column(cavity, 0.25), 0.0, "a crest is no hollow");
    // The shoulder either side of the ridge is where the field turns the other
    // way, so that is where the cavity mask lives.
    assert!(column(cavity, 0.29) > 0.2, "{}", column(cavity, 0.29));
    assert_eq!(column(peaks, 0.29), 0.0);
    // Both are masks: nothing runs past one or below zero.
    for texel in peaks.iter().chain(cavity) {
        assert!((0.0..=1.0).contains(texel), "{texel}");
    }
}

#[test]
fn an_edge_is_one_where_a_mask_steps_and_zero_where_it_is_flat() {
    // A mask that steps from zero to one at u = 0.5, and an edge detect over
    // it. The difference across the neighbourhood is the whole step, so the
    // line is one whatever the radius; a slope would have answered the step
    // divided by the span, which is a number in the hundreds at this
    // resolution. The line is two radii wide, and everything else is dark.
    let radius = 8.0 / N as f32;
    let graph = wired()
        .node("mask", Math::new(MathOp::Step, "u", 0.5))
        .node("n", EdgeDetect::new("mask").radius(radius))
        .output(PbrOutput::new().roughness("n"))
        .into_graph();
    let edge = planes(&graph, N).roughness;
    let column = |u: f32| at(&edge, texel_of(u), N / 2);
    assert_eq!(column(0.5), 1.0, "the step itself");
    assert!(column(0.5 + radius * 0.5) > 0.9);
    assert!(column(0.5 + radius * 1.5) < 1e-6, "past the reach");
    assert!(column(0.25) < 1e-6, "flat ground");
    // The seam is a step as well — zero the far side, one this side — so a
    // filter that clamped instead of wrapping would answer nothing there.
    assert_eq!(column(0.0), 1.0, "the wrap is an edge too");
}

#[test]
fn slope_min_max_preserve_order_and_have_distinct_cache_keys() {
    use ashlar_material::nodes::SlopeMode;
    let graph = wired()
        .node("source", Noise::value().period(8).seed(4))
        .node("guide", Noise::perlin().period(4).seed(7))
        .node(
            "min",
            Blur::slope("source", "guide")
                .radius(0.06)
                .steps(32)
                .slope_mode(SlopeMode::Min),
        )
        .node(
            "max",
            Blur::slope("source", "guide")
                .radius(0.06)
                .steps(32)
                .slope_mode(SlopeMode::Max),
        )
        .node("avg", Blur::slope("source", "guide").radius(0.06).steps(32))
        .output(
            PbrOutput::new()
                .roughness("min")
                .metallic("max")
                .occlusion("avg"),
        )
        .into_graph();
    let output = planes(&graph, N);
    let original = planes(
        &wired()
            .node("source", Noise::value().period(8).seed(4))
            .output(PbrOutput::new().roughness("source"))
            .into_graph(),
        N,
    )
    .roughness;
    let mut changed = 0;
    for (((lo, hi), avg), source) in output
        .roughness
        .iter()
        .zip(&output.metallic)
        .zip(&output.occlusion)
        .zip(original)
    {
        assert!(*lo <= source + 1e-6 && *hi >= source - 1e-6);
        assert!(*lo <= *avg + 1e-6 && *hi >= *avg - 1e-6);
        if hi - lo > 0.1 {
            changed += 1;
        }
    }
    assert!(
        changed > 1000,
        "reductions must not alias one cached filter"
    );
    // Legacy serialized nodes omitted the new field and retain average behavior.
    let legacy: Blur = ron::from_str("(kind:Slope)").unwrap();
    assert_eq!(legacy.slope_mode, SlopeMode::Average);
    let encoded = ron::to_string(&Blur::slope(0.5, 0.2).slope_mode(SlopeMode::Min)).unwrap();
    assert_eq!(
        ron::from_str::<Blur>(&encoded).unwrap().slope_mode,
        SlopeMode::Min
    );
}
