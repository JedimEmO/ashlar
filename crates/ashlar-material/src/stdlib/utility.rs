//! Small utility finishes for complete building bindings.
use super::surfaces::{m, p, remap};
use crate::{
    MaterialGraph, MathOp, Param, PbrOutput,
    nodes::{Mix, Noise},
};

/// Reflective architectural glazing with faint smudges. This is the opaque
/// facade-glass convention used by the building kits; transmission is geometry
/// and renderer policy, not an alpha channel encoded in the material graph.
#[must_use]
pub fn glass() -> MaterialGraph {
    MaterialGraph::builder("library:glass")
        .param(Param::float("smudge", 0.25).range(0.0, 1.0))
        .node(
            "smudge",
            Noise::perlin().periods(8, 16).octaves(3).seed(901),
        )
        .node(
            "roughness",
            m(
                MathOp::Add,
                0.1,
                m(MathOp::Mul, "smudge", m(MathOp::Mul, p("smudge"), 0.16)),
            ),
        )
        .node(
            "color",
            Mix::new(
                [0.025, 0.055, 0.065],
                [0.035, 0.06, 0.067],
                m(MathOp::Mul, "smudge", p("smudge")),
            ),
        )
        .tile_metres([2.0; 2])
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness("roughness")
                .metallic(0.5),
        )
        .into_graph()
}

/// Deep, matte recess behind vents, reveals and open joints.
#[must_use]
pub fn dark_recess() -> MaterialGraph {
    MaterialGraph::builder("library:dark-recess")
        .tile_metres([2.0; 2])
        .output(
            PbrOutput::new()
                .base_color([0.008, 0.011, 0.013])
                .roughness(0.96)
                .metallic(0.0),
        )
        .into_graph()
}

/// The study's acrylic diffuser, without its scene-specific animated clock.
/// Emission is normalized here; the default definition carries its radiance.
#[must_use]
pub fn emissive_strip() -> MaterialGraph {
    MaterialGraph::builder("library:emissive-strip")
        .node("grain", Noise::value().periods(4, 64).seed(51))
        .node("dust", Noise::value().period(128).seed(52))
        .node(
            "diffuser",
            m(
                MathOp::Mul,
                remap("grain", 0.90, 1.0),
                remap("dust", 0.94, 1.02),
            ),
        )
        .node("color", m(MathOp::Mul, [0.62, 0.82, 0.84], "diffuser"))
        .tile_metres([2.25, 0.5])
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness(remap("grain", 0.3, 0.42))
                .metallic(0.0)
                .emissive("diffuser"),
        )
        .into_graph()
}

/// Warm white signage ink, with a colour parameter for a building's lettering.
#[must_use]
pub fn signage_ink() -> MaterialGraph {
    MaterialGraph::builder("library:signage-ink")
        .param(Param::color("color", [0.8, 0.78, 0.7]))
        .tile_metres([1.0; 2])
        .output(
            PbrOutput::new()
                .base_color(p("color"))
                .roughness(0.52)
                .metallic(0.0),
        )
        .into_graph()
}
