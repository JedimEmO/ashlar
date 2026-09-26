//! The helpers the reference reproductions share: the brick, the cobblestone
//! and the grass were reviewed against their references built on exactly
//! these, so they stay as they are rather than follow `surfaces`' own.
use crate::{
    Channel, Input, MaterialGraph, MaterialGraphBuilder,
    MathOp::{self, Add, Div, Floor, Fract, Min, Mul},
    PbrOutput,
    nodes::{Decompose, Invert, Levels, Math, OcclusionFromHeight, Uv},
};

pub(super) const TILE_METRES: f32 = 2.0;
pub(super) fn p(name: &str) -> Input {
    Input::param(name)
}
pub(super) fn m(op: MathOp, a: impl Into<Input>, b: impl Into<Input>) -> Math {
    Math::new(op, a, b)
}
pub(super) fn remap(input: impl Into<Input>, low: f32, high: f32) -> Levels {
    Levels::new(input).out_range(low, high)
}
pub(super) fn finish(g: MaterialGraphBuilder, output: PbrOutput, scale: f32) -> MaterialGraph {
    g.node("physical_height", m(Mul, "height", scale / TILE_METRES))
        .node(
            "ao",
            OcclusionFromHeight::new("physical_height").radius(0.015),
        )
        .output(output.height("height").normal_strength(scale / TILE_METRES))
        // The same two metres the relief just above was divided by, said in the
        // graph rather than only in this file: `scale / TILE_METRES` is metres
        // of relief per repeat and is only that at this repeat size.
        .tile_metres([TILE_METRES; 2])
        .build()
        .expect("reference graph is valid")
        .into_graph()
}

// Distances to rectangular cell edges in UV, so joints have the same world
// width on both axes. IDs come from the identical running-bond partition.
pub(super) fn rectangles(
    g: MaterialGraphBuilder,
    columns: f32,
    rows: f32,
    offset: f32,
) -> MaterialGraphBuilder {
    g.node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("row", m(Mul, "v", rows))
        .node("shift", m(Mul, Math::unary(Floor, "row"), offset))
        .node(
            "x",
            Math::unary(Fract, m(Add, m(Mul, "u", columns), "shift")),
        )
        .node("y", Math::unary(Fract, "row"))
        .node("dx", m(Div, m(Min, "x", Invert::new("x")), columns))
        .node("dy", m(Div, m(Min, "y", Invert::new("y")), rows))
        .node("edge_distance", m(Min, "dx", "dy"))
}
