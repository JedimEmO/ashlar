//! Lime render and its intact and damaged finishes.
use super::surfaces::{CHANNELS, finish, m, p, remap};
use crate::{
    MaterialGraph, MathOp, Param, ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Clamp, Invert, Levels, Math, Mix, Noise, OcclusionFromHeight, Subgraph, Voronoi,
        VoronoiOutput, Warp,
    },
};

/// A painted lime render: thrown aggregate, sparse pocks and hairline cracks.
///
/// The thrown coat is two lattices of warped cellular lumps, about 8 and 16 mm
/// across, flattened where a float passed over it; a third lattice leaves a
/// pock in about one cell in four. A paint film fills the finest grain, so the
/// finish is a matt paint rather than raw lime. `smoothness` removes the
/// aggregate and the pocks for an interior finish and leaves the slow trowel
/// undulation. Height is in a 20 mm range over a two metre repeat.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn lime_render() -> MaterialGraph {
    use MathOp::{Add, Div, Mul, Step, Sub};
    let g = MaterialGraph::builder("substances:lime-render")
        .param(Param::float("smoothness", 0.0).range(0.0, 1.0))
        .param(Param::color("color", [0.5, 0.5, 0.48]))
        .node("trowel", Noise::perlin().period(8).octaves(2).seed(401))
        // Where a float went over the thrown coat and knocked its tops off.
        .node(
            "floated",
            Levels::new(Noise::perlin().period(4).octaves(2).seed(443)).in_range(0.6, 0.8),
        )
        .node(
            "wobble",
            remap(
                Noise::perlin().period(64).octaves(2).seed(449),
                -0.004,
                0.004,
            ),
        )
        .node("grain", Noise::perlin().period(512).seed(409))
        .node(
            "grit_cells",
            Warp::new(Voronoi::new().period(512).jitter(0.95).seed(453), "wobble").amount(1.0),
        )
        .node(
            "fine_cells",
            Warp::new(Voronoi::new().period(256).jitter(0.9).seed(457), "wobble").amount(1.0),
        )
        .node(
            "coarse_cells",
            Warp::new(Voronoi::new().period(64).jitter(0.9).seed(463), "wobble").amount(1.0),
        )
        // Stucco: rounded blobs, one per cell, a parabolic dome of radius a
        // little over half a cell, so neighbours touch along their edges and
        // only the corners where three meet stay open. Two lattices, 4 and
        // 8 mm, united by the higher and roughened by a millimetre grain;
        // where neither reaches is a pocket, 1 to 3 mm across and as deep as
        // the blobs are tall.
        .node(
            "grit_lumps",
            Clamp::new(m(
                Sub,
                1.0,
                m(Mul, m(Div, "grit_cells", 0.58), m(Div, "grit_cells", 0.58)),
            )),
        )
        .node(
            "fine_lumps",
            Clamp::new(m(
                Sub,
                1.0,
                m(Mul, m(Div, "fine_cells", 0.58), m(Div, "fine_cells", 0.58)),
            )),
        )
        .node(
            "coarse_lumps",
            Invert::new(Clamp::new(m(Mul, "coarse_cells", 1.5))),
        )
        .node(
            "blobs",
            m(
                Add,
                m(MathOp::Max, "grit_lumps", m(Mul, "fine_lumps", 0.8)),
                remap("grain", -0.08, 0.08),
            ),
        )
        // The pockets carry the stucco's contrast: open ground between blobs.
        .node("pockets", Clamp::new(m(Div, m(Sub, 0.34, "blobs"), 0.28)))
        .node(
            "aggregate",
            m(Add, m(Mul, "blobs", 0.7), m(Mul, "coarse_lumps", 0.3)),
        )
        // Pocks: a pit in roughly one cell in eight of a 31 mm lattice, 3 to
        // 9 mm across by the cell's own hash, pulled out of round by the same
        // wobble.
        .node(
            "pock_cells",
            Warp::new(Voronoi::new().period(64).jitter(0.8).seed(467), "wobble").amount(1.0),
        )
        .node(
            "pock_seed",
            Voronoi::new()
                .period(64)
                .jitter(0.8)
                .seed(467)
                .output(VoronoiOutput::Cell),
        )
        .node(
            "pocks",
            m(
                Mul,
                m(Step, "pock_seed", 0.88),
                Clamp::new(m(
                    Div,
                    m(
                        Sub,
                        remap(
                            Math::unary(MathOp::Fract, m(Mul, "pock_seed", 7.31)),
                            0.05,
                            0.15,
                        ),
                        "pock_cells",
                    ),
                    0.05,
                )),
            ),
        )
        // Hairlines: the boundaries of a lattice of tall cells, so the lines
        // run mostly up the wall, warped off the lattice, and kept only where
        // a coarse field allows, so each one starts and dies out on its own.
        .node(
            "meander",
            m(
                MathOp::Add,
                remap(Noise::perlin().period(16).octaves(2).seed(481), -0.02, 0.02),
                remap(
                    Noise::perlin().period(64).octaves(3).seed(487),
                    -0.0055,
                    0.0055,
                ),
            ),
        )
        .node(
            "crack_cells",
            Warp::new(
                Voronoi::new()
                    .periods(8, 2)
                    .jitter(0.9)
                    .seed(479)
                    .output(VoronoiOutput::Edge),
                "meander",
            )
            .amount(1.0),
        )
        .node(
            "crack_bias",
            Levels::new(Noise::perlin().period(4).octaves(2).seed(419)).in_range(0.5, 0.64),
        )
        .node(
            "crack_line",
            Clamp::new(m(Div, m(Sub, 0.006, "crack_cells"), 0.004)),
        )
        // A skim coat hides most of them: an interior finish keeps a faint
        // trace rather than every hairline the exterior shows.
        .node(
            "cracks",
            m(
                Mul,
                m(Mul, "crack_line", "crack_bias"),
                m(Sub, 1.0, m(Mul, p("smoothness"), 0.85)),
            ),
        )
        .node("rough_coat", Invert::new(p("smoothness")))
        .node(
            "thrown",
            m(Mul, m(Sub, "aggregate", 0.35), remap("floated", 0.22, 0.06)),
        )
        .node(
            "texture",
            m(
                Mul,
                "rough_coat",
                m(
                    Sub,
                    m(Add, "thrown", remap("grain", -0.005, 0.005)),
                    m(Mul, "pocks", 0.07),
                ),
            ),
        )
        .node(
            "height",
            m(
                Sub,
                m(Add, remap("trowel", 0.488, 0.512), "texture"),
                m(Mul, "cracks", 0.05),
            ),
        )
        .node(
            "mottle",
            remap(Noise::perlin().period(8).octaves(4).seed(471), 0.93, 1.04),
        )
        // Dust held in the hollows between the lumps: a thrown wall is never
        // as white in its pits as on its tops.
        .node(
            "hollow_grime",
            m(
                Add,
                1.0,
                m(
                    Mul,
                    m(Mul, "rough_coat", remap("floated", 1.0, 0.5)),
                    m(
                        Add,
                        m(Mul, "pockets", -0.6),
                        remap("aggregate", -0.06, 0.02),
                    ),
                ),
            ),
        )
        .node(
            "color",
            m(
                Mul,
                p("color"),
                m(
                    Mul,
                    m(
                        Mul,
                        m(Mul, "mottle", remap("trowel", 0.985, 1.015)),
                        "hollow_grime",
                    ),
                    m(
                        Mul,
                        remap(m(Mul, "pocks", "rough_coat"), 1.0, 0.94),
                        remap("cracks", 1.0, 0.32),
                    ),
                ),
            ),
        )
        .node(
            "roughness",
            Mix::new(
                m(
                    Add,
                    m(Add, remap("grain", 0.6, 0.68), m(Mul, "pocks", 0.14)),
                    m(Mul, "cracks", 0.2),
                ),
                m(Add, 0.52, m(Mul, "cracks", 0.2)),
                p("smoothness"),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("cracks", "cracks"),
        2.0,
        0.02,
    )
}

/// White exterior plaster; set `smoothness` to one for a smooth interior wall.
#[must_use]
pub fn plaster() -> MaterialGraph {
    let g = MaterialGraph::builder("library:plaster")
        .param(Param::float("smoothness", 0.0).range(0.0, 1.0))
        .layer("rough", Subgraph::new("substances:lime-render"), &CHANNELS)
        .layer(
            "smooth",
            Subgraph::new("substances:lime-render").param("smoothness", ParamValue::Float(1.0)),
            &CHANNELS,
        )
        .node(
            "height",
            Mix::new("rough.height", "smooth.height", p("smoothness")),
        )
        .node(
            "roughness",
            Mix::new("rough.roughness", "smooth.roughness", p("smoothness")),
        )
        .node(
            "color",
            Mix::new("rough.base_color", "smooth.base_color", p("smoothness")),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0),
        2.0,
        0.02,
    )
}

/// Lime render lost in irregular patches over the library's reference brick.
///
/// Brick height uses a 40 mm range, so the lime's 20 mm relief is halved before
/// adding it. The coating adds 4 mm on intact regions. One loss mask chooses
/// colour, roughness and height; at `loss = 1` the brick is recovered exactly.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn damaged_plaster() -> MaterialGraph {
    use MathOp::{Add, Mul, Sub};
    let g = MaterialGraph::builder("library:damaged-plaster")
        .param(Param::float("loss", 0.45).range(0.0, 1.0))
        .layer(
            "brick",
            Subgraph::new("library:brick"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("joint_depth".into()),
            ],
        )
        // The pre-2026-09-24 lime tone: a fresh white render over old brick.
        .layer(
            "lime",
            Subgraph::new("substances:lime-render")
                .param("color", ParamValue::Color([0.62, 0.615, 0.59])),
            &CHANNELS,
        )
        .node("bond", Noise::perlin().period(4).octaves(3).seed(431))
        .node("edge", Noise::perlin().period(128).seed(433))
        .node(
            "failure",
            remap(
                m(
                    Sub,
                    m(Add, remap("bond", 0.05, 0.95), remap("edge", -0.04, 0.04)),
                    m(Mul, "brick.joint_depth", 0.012),
                ),
                0.10,
                0.90,
            ),
        )
        // Two coats fail in two steps. The brick shows where `failure` falls
        // below `loss`; the finish coat breaks back further than the scratch
        // coat under it, so a band of the sandy base coat shows between the
        // white and the brick, its outer edge jagged by a noise of its own.
        // At `loss = 1` both are gone everywhere: `failure` never reaches 0.9,
        // so both thresholds clear and every mix below returns the brick bit
        // for bit.
        .node("step_edge", Noise::perlin().period(64).octaves(2).seed(439))
        .node(
            "scratch_remaining",
            Clamp::new(m(MathOp::Div, m(Sub, "failure", p("loss")), 0.02)),
        )
        .node(
            "remaining",
            Clamp::new(m(
                MathOp::Div,
                m(
                    Sub,
                    m(Sub, "failure", remap("step_edge", 0.01, 0.065)),
                    p("loss"),
                ),
                0.015,
            )),
        )
        .node("sand", Noise::perlin().period(256).octaves(2).seed(491))
        .node("sand_tone", Noise::perlin().period(16).octaves(3).seed(493))
        .node(
            "scratch_color",
            m(
                Mul,
                [0.36, 0.29, 0.19],
                m(
                    Mul,
                    remap("sand", 0.82, 1.1),
                    remap("sand_tone", 0.88, 1.08),
                ),
            ),
        )
        .node(
            "scratch_height",
            m(
                Add,
                "brick.height",
                m(Add, 0.05, remap("sand", -0.012, 0.012)),
            ),
        )
        .node(
            "thickness",
            m(Add, 0.10, m(Mul, m(Sub, "lime.height", 0.5), 0.5)),
        )
        .node("coated_height", m(Add, "brick.height", "thickness"))
        .node(
            "height",
            Mix::new(
                Mix::new("brick.height", "scratch_height", "scratch_remaining"),
                "coated_height",
                "remaining",
            ),
        )
        // Weathered: the finish is dirtier toward its broken edge, where water
        // gets behind it, and in a broad wash over the wall.
        .node("wash", Noise::perlin().period(4).octaves(3).seed(497))
        .node(
            "edge_dirt",
            Clamp::new(m(MathOp::Div, m(Sub, "failure", p("loss")), 0.16)),
        )
        .node(
            "lime_dirty",
            m(
                Mul,
                "lime.base_color",
                m(Mul, remap("wash", 0.8, 1.0), remap("edge_dirt", 0.78, 1.0)),
            ),
        )
        .node(
            "color",
            Mix::new(
                Mix::new("brick.base_color", "scratch_color", "scratch_remaining"),
                m(Mul, "lime_dirty", [1.0, 0.98, 0.93]),
                "remaining",
            ),
        )
        .node(
            "roughness",
            Mix::new(
                Mix::new("brick.roughness", 0.93, "scratch_remaining"),
                "lime.roughness",
                "remaining",
            ),
        );
    g.node("physical_height", m(Mul, "height", 0.02))
        .node(
            "ao",
            OcclusionFromHeight::new("physical_height").radius(0.015),
        )
        .tile_metres([2.0; 2])
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness("roughness")
                .metallic(0.0)
                .height("height")
                .occlusion("ao")
                .normal_strength(0.02)
                .extra("remaining", "remaining"),
        )
        .into_graph()
}
