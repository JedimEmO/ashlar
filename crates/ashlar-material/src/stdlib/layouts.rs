//! Reusable unit layouts. Every exported field reads the same partition.
use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, PbrOutput,
    nodes::{Clamp, Decompose, Invert, Levels, Math, Noise, Uv},
};

fn m(op: MathOp, a: impl Into<Input>, b: impl Into<Input>) -> Math {
    Math::new(op, a, b)
}
fn p(name: &str) -> Input {
    Input::param(name)
}

/// Rectangular slabs with stable identity, an edge distance and recessed joints.
///
/// Counts round down to whole units in 1..=64; the bond offset rounds down to
/// a multiple of 1/rows so its accumulated shift closes at the repeat seam.
/// Distances and joint width are in UV, keeping the same physical joint width
/// on both axes even for long rectangles. `irregularity` erodes an edge by at
/// most 3 mm over a two metre repeat, without changing the owning unit.
///
/// This graph draws no substance. Its extras are `unit_id`, `mask`,
/// `edge_distance`, `joint_depth`, `local_u` and `local_v`. `mask` includes a
/// small bevel, and `joint_depth` is its complement, so a caller can blend
/// substrate, grout and height using the same ownership decision.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the layout's one shared coordinate partition"
)]
pub fn slab_lattice() -> MaterialGraph {
    use MathOp::{Add, Div, Floor, Fract, Min, Mul, Sin, Sub};
    MaterialGraph::builder("layouts:slab-lattice")
        .param(Param::int("columns", 4).range(1.0, 64.0))
        .param(Param::int("rows", 4).range(1.0, 64.0))
        .param(Param::float("offset", 0.0).range(0.0, 1.0))
        .param(Param::float("joint_width", 0.004).range(0.0, 0.03))
        .param(Param::float("bevel", 0.0015).range(0.0002, 0.01))
        .param(Param::float("irregularity", 0.3).range(0.0, 1.0))
        .node(
            "columns",
            Math::unary(Floor, Clamp::new(p("columns")).range(1.0, 64.0)),
        )
        .node(
            "rows",
            Math::unary(Floor, Clamp::new(p("rows")).range(1.0, 64.0)),
        )
        .node(
            "offset",
            m(Div, Math::unary(Floor, m(Mul, p("offset"), "rows")), "rows"),
        )
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("row", m(Mul, "v", "rows"))
        .node(
            "row_id",
            m(
                Mul,
                Math::unary(Fract, m(Div, Math::unary(Floor, "row"), "rows")),
                "rows",
            ),
        )
        .node(
            "across",
            m(Add, m(Mul, "u", "columns"), m(Mul, "row_id", "offset")),
        )
        .node(
            "column_id",
            Math::unary(
                Floor,
                m(
                    Mul,
                    Math::unary(Fract, m(Div, "across", "columns")),
                    "columns",
                ),
            ),
        )
        .node("local_u", Math::unary(Fract, "across"))
        .node("local_v", Math::unary(Fract, "row"))
        .node(
            "dx",
            m(Div, m(Min, "local_u", Invert::new("local_u")), "columns"),
        )
        .node(
            "dy",
            m(Div, m(Min, "local_v", Invert::new("local_v")), "rows"),
        )
        .node("edge_distance", m(Min, "dx", "dy"))
        .node(
            "unit_id",
            Math::unary(
                Fract,
                m(
                    Mul,
                    Math::unary(
                        Sin,
                        m(
                            Add,
                            m(Add, m(Mul, "column_id", 0.1731), m(Mul, "row_id", 0.7549)),
                            0.391,
                        ),
                    ),
                    4375.31,
                ),
            ),
        )
        .node("edge_grain", Noise::perlin().period(128).seed(67))
        .node(
            "chips",
            m(
                Mul,
                p("irregularity"),
                m(
                    Add,
                    Levels::new("edge_grain").out_range(0.0, 0.001),
                    m(Mul, "unit_id", 0.0005),
                ),
            ),
        )
        .node(
            "inside",
            m(
                Sub,
                "edge_distance",
                m(Add, m(Mul, p("joint_width"), 0.5), "chips"),
            ),
        )
        .node("mask", Clamp::new(m(Div, "inside", p("bevel"))))
        .node("joint_depth", Invert::new("mask"))
        .output(
            PbrOutput::new()
                .extra("unit_id", "unit_id")
                .extra("mask", "mask")
                .extra("edge_distance", "edge_distance")
                .extra("joint_depth", "joint_depth")
                .extra("local_u", "local_u")
                .extra("local_v", "local_v"),
        )
        .into_graph()
}
