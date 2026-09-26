//! Rolled steel, a reusable paint film, and weathered sheet finishes.
use super::surfaces::{CHANNELS, finish, m, p, remap};
use crate::{
    Channel, MaterialGraph, MaterialGraphBuilder, MathOp, Param, ParamValue, PbrOutput,
    SurfaceOutput,
    nodes::{
        Blur, Clamp, Combine2, Decompose, GraphInput, Invert, Levels, Math, Mix, Noise, Pattern,
        PatternKind, Scratches, Subgraph, Transform, Uv, Voronoi, VoronoiOutput, Warp,
    },
};

/// Rolled sheet steel: smudged mill-scale tone, vertical brushing and faint
/// handling scratches, in a 20 mm height range over a two metre repeat.
///
/// The tone is two scales of blotch rather than one noise: a broad, soft
/// mottle that the eye reads as the sheet's own unevenness, and tighter
/// smudges where it has been handled. Roughness follows both, so a smudge is
/// duller as well as darker and the highlight breaks up across the sheet
/// instead of sitting on it like a mirror. `unit_id` shifts a whole sheet.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn steel_substance() -> MaterialGraph {
    use MathOp::{Add, Max, Mul, Sub};
    let g = MaterialGraph::builder("substances:steel")
        .param(Param::color("color", [0.40, 0.405, 0.41]))
        .node("unit", GraphInput::float("unit_id", 0.5))
        .node(
            "mottle",
            Noise::perlin()
                .period(4)
                .octaves(5)
                .persistence(0.6)
                .seed(503),
        )
        .node(
            "scale_tone",
            Levels::new(Noise::perlin().period(8).octaves(4).seed(511)).in_range(0.5, 0.78),
        )
        .node(
            "smudge",
            Levels::new(Noise::perlin().period(16).octaves(4).seed(513)).in_range(0.56, 0.8),
        )
        .node(
            "brush",
            Noise::perlin().periods(256, 2).octaves(2).seed(509),
        )
        .node("brush_fine", Noise::perlin().periods(512, 16).seed(517))
        .node(
            "lines",
            m(
                Add,
                remap("brush", -1.0, 1.0),
                remap("brush_fine", -0.6, 0.6),
            ),
        )
        .node(
            "handling",
            Scratches::new()
                .count(1024)
                .length(0.035)
                .width(0.00045)
                .angle(90.0)
                .angle_spread(30.0)
                .seed(519),
        )
        .node(
            "stray",
            Scratches::new()
                .count(128)
                .length(0.06)
                .width(0.0005)
                .angle_spread(180.0)
                .seed(521),
        )
        .node(
            "scratch",
            m(Max, m(Mul, "handling", 0.7), m(Mul, "stray", 0.45)),
        )
        .node(
            "tone",
            m(
                Mul,
                m(Mul, remap("mottle", 0.4, 1.12), remap("unit", 0.93, 1.07)),
                m(
                    Add,
                    m(Sub, 1.0, m(Mul, "smudge", 0.26)),
                    m(Mul, "lines", 0.06),
                ),
            ),
        )
        .node("rolled", m(Mul, p("color"), "tone"))
        .node(
            "scaled",
            Mix::new(
                "rolled",
                m(Mul, "rolled", [0.62, 0.64, 0.69]),
                m(Mul, "scale_tone", 0.7),
            ),
        )
        .node(
            "color",
            Mix::new("scaled", m(Mul, p("color"), 1.18), m(Mul, "scratch", 0.5)),
        )
        .node(
            "roughness",
            Clamp::new(m(
                Sub,
                m(
                    Add,
                    m(Add, remap("mottle", 0.3, 0.5), m(Mul, "smudge", 0.16)),
                    m(Add, m(Mul, "scale_tone", 0.08), m(Mul, "lines", 0.05)),
                ),
                m(Mul, "scratch", 0.14),
            )),
        )
        .node(
            "height",
            m(
                Sub,
                m(Add, remap("mottle", 0.497, 0.503), m(Mul, "lines", 0.0012)),
                m(Mul, "scratch", 0.003),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(1.0),
        2.0,
        0.02,
    )
}

/// The library's bare steel finish.
#[must_use]
pub fn steel() -> MaterialGraph {
    let g = MaterialGraph::builder("library:steel")
        .layer("steel", Subgraph::new("substances:steel"), &CHANNELS)
        .node("height", m(MathOp::Add, "steel.height", 0.0));
    finish(
        g,
        PbrOutput::new()
            .base_color("steel.base_color")
            .roughness("steel.roughness")
            .metallic(1.0),
        2.0,
        0.02,
    )
}

/// A dielectric paint film over caller-supplied channels. Loss is shared by
/// colour, roughness, metalness and thickness; full loss recovers the substrate.
/// `thickness` is in the caller's normalized height units (default 0.1 mm
/// when the caller uses the library's 20 mm range).
#[must_use]
pub fn paint_film() -> MaterialGraph {
    use MathOp::{Add, Mul};
    MaterialGraph::builder("weathering:paint_film")
        .param(Param::float("thickness", 0.005).range(0.0, 0.1))
        .param(Param::float("paint_roughness", 0.3).range(0.0, 1.0))
        .node(
            "base_color",
            GraphInput::color("base_color", [0.52, 0.54, 0.55]),
        )
        .node("roughness", GraphInput::float("roughness", 0.3))
        .node("metallic", GraphInput::float("metallic", 1.0))
        .node("height", GraphInput::float("height", 0.5))
        .node(
            "paint_color",
            GraphInput::color("paint_color", [0.5, 0.025, 0.018]),
        )
        .node("loss", GraphInput::float("loss_mask", 0.0))
        .node("remaining", Invert::new(Clamp::new("loss")))
        .node("color", Mix::new("base_color", "paint_color", "remaining"))
        .node(
            "rough",
            Mix::new("roughness", p("paint_roughness"), "remaining"),
        )
        .node("metal", Mix::new("metallic", 0.0, "remaining"))
        .node(
            "coated",
            Clamp::new(m(Add, "height", m(Mul, "remaining", p("thickness")))),
        )
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness("rough")
                .metallic("metal")
                .height("coated")
                .normal_strength(0.01)
                .extra("remaining", "remaining"),
        )
        .into_graph()
}

/// Chipped and scratched red enamel over grey primer on sheet steel.
///
/// Wear is one field read at three thresholds, so every chip is nested the
/// way a real one is: the topcoat goes first, a ring of dark primer stands
/// round the hole, and bare steel shows only in the middle. The field is a
/// jagged fractal biased by a broad habitat, so chips cluster where a sheet
/// was knocked rather than falling evenly like confetti, and it is lifted
/// along the crests of any `profile` the caller wires in. Scratches are short
/// and bent; most only scuff the enamel pale and the deepest reach the steel.
///
/// `wear_bias` scales `wear` from outside, which is how the corrugated sheet
/// drives it from its own `age`.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn painted_metal() -> MaterialGraph {
    use MathOp::{Add, Max, Mul, Sub};
    let g = MaterialGraph::builder("library:painted-metal")
        .param(Param::color("color", [0.60, 0.022, 0.015]))
        .param(Param::float("wear", 0.55).range(0.0, 1.0))
        .node("profile", GraphInput::float("profile", 0.5))
        .node("wear_bias", GraphInput::float("wear_bias", 1.0))
        .node("loss_bias", GraphInput::float("loss_bias", 0.0))
        .layer("steel", Subgraph::new("substances:steel"), &CHANNELS)
        .node(
            "substrate_height",
            m(Add, "profile", m(Sub, "steel.height", 0.5)),
        )
        // Where the sheet was knocked: broad, soft, and higher on a crest.
        .node(
            "habitat",
            m(
                Add,
                remap(Noise::perlin().period(4).octaves(3).seed(531), -0.3, 0.3),
                m(Mul, m(Sub, "profile", 0.5), 0.5),
            ),
        )
        // The chip edge: a jagged fractal with a finer fleck on top of it.
        .node(
            "fracture",
            m(
                Add,
                m(
                    Mul,
                    Noise::perlin()
                        .period(8)
                        .octaves(6)
                        .persistence(0.58)
                        .seed(523),
                    0.75,
                ),
                m(Mul, Noise::perlin().period(64).octaves(3).seed(527), 0.25),
            ),
        )
        .node(
            "exposure",
            m(
                Add,
                m(Add, "fracture", "habitat"),
                m(
                    Add,
                    m(Mul, m(Mul, p("wear"), "wear_bias"), 0.3),
                    "loss_bias",
                ),
            ),
        )
        .node(
            "topcoat_lost",
            Levels::new("exposure").in_range(0.815, 0.83),
        )
        .node("primer_lost", Levels::new("exposure").in_range(0.86, 0.875))
        // Short bent scratches: many fine ones, a few longer.
        .node(
            "bend",
            Combine2::new(
                remap(Noise::perlin().period(2).seed(541), -1.0, 1.0),
                remap(Noise::perlin().period(2).seed(543), -1.0, 1.0),
            ),
        )
        .node(
            "fine_scratches",
            Warp::new(
                Scratches::new()
                    .count(1024)
                    .length(0.05)
                    .width(0.0005)
                    .angle_spread(180.0)
                    .seed(521),
                "bend",
            )
            .amount(0.012),
        )
        .node(
            "long_scratches",
            Warp::new(
                Scratches::new()
                    .count(256)
                    .length(0.16)
                    .width(0.0007)
                    .angle_spread(180.0)
                    .seed(529),
                "bend",
            )
            .amount(0.035),
        )
        .node(
            "scratch_depth",
            Levels::new(Noise::perlin().period(16).octaves(3).seed(547)).in_range(0.4, 0.85),
        )
        .node(
            "scratch",
            m(
                Mul,
                m(Max, m(Mul, "fine_scratches", 0.45), "long_scratches"),
                m(
                    Mul,
                    "scratch_depth",
                    m(Add, 0.15, m(Mul, m(Mul, p("wear"), "wear_bias"), 1.4)),
                ),
            ),
        )
        .node("through", Levels::new("scratch").in_range(0.75, 0.9))
        .layer(
            "primer",
            Subgraph::new("weathering:paint_film")
                .input("base_color", "steel.base_color")
                .input("roughness", "steel.roughness")
                .input("metallic", "steel.metallic")
                .input("height", "substrate_height")
                .input("paint_color", [0.035, 0.036, 0.035])
                .input("loss_mask", m(Max, "primer_lost", "through"))
                .param("thickness", ParamValue::Float(0.006))
                .param("paint_roughness", ParamValue::Float(0.62)),
            &CHANNELS,
        )
        .node(
            "enamel",
            m(
                Mul,
                p("color"),
                remap(Noise::perlin().period(4).octaves(3).seed(551), 0.86, 1.06),
            ),
        )
        .layer(
            "paint",
            Subgraph::new("weathering:paint_film")
                .input("base_color", "primer.base_color")
                .input("roughness", "primer.roughness")
                .input("metallic", "primer.metallic")
                .input("height", "primer.height")
                .input("paint_color", "enamel")
                .input("loss_mask", m(Max, "topcoat_lost", "through"))
                .param("thickness", ParamValue::Float(0.009))
                .param("paint_roughness", ParamValue::Float(0.26)),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("remaining".into()),
            ],
        )
        // A scuff lifts the enamel pale and matt without opening it.
        .node("scuff", m(Mul, "scratch", "paint.remaining"))
        .node(
            "scuffed",
            Mix::new(
                "paint.base_color",
                [0.62, 0.36, 0.34],
                m(Mul, "scuff", 0.75),
            ),
        )
        .node(
            "grime",
            m(
                Mul,
                Levels::new(Noise::perlin().period(8).octaves(4).seed(557)).in_range(0.5, 0.85),
                0.3,
            ),
        )
        .node("color", m(Mul, "scuffed", m(Sub, 1.0, "grime")))
        .node(
            "roughness",
            Clamp::new(m(
                Add,
                m(
                    Add,
                    "paint.roughness",
                    m(Mul, "paint.remaining", remap("orange_peel", -0.04, 0.04)),
                ),
                m(Add, m(Mul, "scuff", 0.3), m(Mul, "grime", 0.5)),
            )),
        )
        .node("orange_peel", Noise::perlin().period(256).seed(559))
        .node(
            "height",
            Clamp::new(m(
                Sub,
                m(
                    Add,
                    "paint.height",
                    m(
                        Mul,
                        "paint.remaining",
                        remap("orange_peel", -0.0012, 0.0012),
                    ),
                ),
                m(Mul, "scratch", 0.004),
            )),
        )
        .node("loss", m(Max, "topcoat_lost", "through"));
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic("paint.metallic")
            .extra("loss", "loss"),
        2.0,
        0.02,
    )
}

/// Sheet steel rusting granularly, at several scales at once. Age zero is the
/// bare steel exactly; one is a sheet scaled almost all over.
///
/// A coarse field (where water sat) and a mid-scale one (how it spread) sum
/// to a *density*, and the density is a threshold on a fine granular field:
/// where it is low only the highest grains have turned, a sparse speckle;
/// where it is high the grains have merged into solid scale. So a patch has
/// no outline — it thins into specks and then into a haze — and nowhere is
/// quite clean, because the density also stains the bare steel between with
/// a brownish oxidation film. Dense cores are dark brown scale; the granular
/// margins are fresh orange. The rust compound cuts pits under the scale.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn rusted_steel() -> MaterialGraph {
    use MathOp::{Add, Mul, Sub};
    let g = MaterialGraph::builder("library:rusted-steel")
        .param(Param::float("age", 0.55).range(0.0, 1.0))
        .layer("steel", Subgraph::new("substances:steel"), &CHANNELS)
        // Exactly zero at age zero, whatever the fields below say.
        .node("aged", Clamp::new(m(Mul, p("age"), 100.0)))
        .node(
            "coarse",
            Noise::perlin()
                .period(4)
                .octaves(3)
                .persistence(0.55)
                .seed(541),
        )
        .node(
            "spread",
            Noise::perlin()
                .periods(16, 8)
                .octaves(4)
                .persistence(0.6)
                .seed(563),
        )
        .node(
            "raw_density",
            m(Add, m(Mul, "coarse", 0.55), m(Mul, "spread", 0.45)),
        )
        // Centred on a half at the middle age, stretched to span sparse to
        // solid across the sheet, and moved bodily by `age`.
        .node(
            "density",
            Clamp::new(m(
                Add,
                m(
                    Mul,
                    m(
                        Add,
                        m(Sub, "raw_density", 0.5),
                        m(Mul, m(Sub, p("age"), 0.47), 0.5),
                    ),
                    2.5,
                ),
                0.5,
            )),
        )
        // The grain the scale forms in: pitting-sized specks on a lacy mid
        // scale, so a threshold on it breaks into specks, not smooth blobs.
        .node(
            "grains",
            m(
                Add,
                m(
                    Add,
                    m(Mul, Noise::perlin().period(128).octaves(2).seed(569), 0.35),
                    m(Mul, Noise::perlin().period(256).seed(577), 0.35),
                ),
                m(
                    Mul,
                    Noise::perlin()
                        .period(32)
                        .octaves(3)
                        .persistence(0.6)
                        .seed(571),
                    0.3,
                ),
            ),
        )
        .node(
            "scale",
            m(
                Mul,
                Levels::new(m(Add, "grains", "density")).in_range(1.0, 1.06),
                "aged",
            ),
        )
        .node("seed", m(Mul, "scale", 1.0))
        .layer(
            "rust",
            Subgraph::new("weathering:rust")
                .input("base_color", "steel.base_color")
                .input("roughness", "steel.roughness")
                .input("metallic", "steel.metallic")
                .input("height", "steel.height")
                .input("seed_mask", "seed")
                .param("spread", ParamValue::Float(0.0005))
                .param("pitting", ParamValue::Float(0.03))
                .param("blister", ParamValue::Float(0.0)),
            &rust_channels(),
        )
        .node("coverage", m(Mul, "scale", 1.0))
        .node("core", Levels::new("density").in_range(0.5, 0.95))
        .node(
            "haze",
            m(
                Mul,
                m(
                    Mul,
                    remap(Levels::new("density").in_range(0.0, 0.7), 0.3, 0.85),
                    remap("grains", 0.35, 1.0),
                ),
                "aged",
            ),
        );
    oxide_finish(
        g,
        "steel",
        2.0,
        Oxide {
            holes: None,
            run: 0.03,
        },
    )
}

/// 75 mm sinusoidal corrugations, 17 mm crest to trough, sixteen to a 1.2 m
/// repeat, with a screw on every other crest along two purlin lines.
///
/// `profile` is the sheet's height in the caller's `0..=1` units; `fixings`
/// is one in a screw hole and `washers` one under its head. The screws sit on
/// crests, as they do on a roof.
#[must_use]
pub fn corrugation() -> MaterialGraph {
    use MathOp::{Add, Mul, Sub};
    MaterialGraph::builder("layouts:corrugation")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node(
            "wave",
            remap(Math::unary(MathOp::Sin, m(Mul, "u", 16.0)), 0.05, 0.9).in_range(-1.0, 1.0),
        )
        // Cell centres on u = (2i + 1) / 16 are mid-flank; a quarter of a
        // corrugation moves them onto the crest, where sin(16 u) is one.
        .node(
            "screw_cells",
            Transform::new(
                Voronoi::new()
                    .period(8)
                    .jitter(0.0)
                    .output(VoronoiOutput::Distance),
            )
            .translate(-0.25 / 16.0, 0.0),
        )
        // One screw row in four: the second half of every four rows (the y(2)
        // square wave) times the odd rows (the y(4) one) leaves row 3.
        .node(
            "purlin",
            m(
                Mul,
                Pattern::new(PatternKind::Square).y(2),
                Pattern::new(PatternKind::Square).y(4),
            ),
        )
        .node(
            "fixings",
            m(
                Mul,
                Invert::new(Levels::new("screw_cells").in_range(0.03, 0.042)),
                "purlin",
            ),
        )
        .node(
            "washers",
            m(
                Mul,
                Invert::new(Levels::new("screw_cells").in_range(0.06, 0.075)),
                "purlin",
            ),
        )
        .node(
            "profile",
            m(
                Sub,
                m(Add, "wave", m(Mul, "washers", 0.02)),
                m(Mul, "fixings", 0.06),
            ),
        )
        .tile_metres([1.2; 2])
        .output(
            PbrOutput::new()
                .extra("profile", "profile")
                .extra("fixings", "fixings")
                .extra("washers", "washers")
                .extra("mask", "wave"),
        )
        .into_graph()
}

/// Painted corrugated sheet on one `age` control: clean pale enamel at zero
/// (`CorrugatedSteel007A`), the paint lost in vertical tears down the sheet
/// with rust in every opening by the middle (007B), and most of the sheet
/// scaled over with streaks of paint left at one (007C).
///
/// Paint loss is the painted metal's own chip field pushed hard by a pattern
/// stretched six to eight times down the sheet, and lifted on the crests,
/// where paint is knocked, and in the troughs, where water runs. The rust
/// seeds wherever the paint is gone and round every screw, and bleeds in long
/// streaks down the troughs below.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn corrugated_steel() -> MaterialGraph {
    use MathOp::{Abs, Add, Max, Min, Mul, Sub};
    let g = MaterialGraph::builder("library:corrugated-steel")
        .param(Param::float("age", 0.55).range(0.0, 1.0))
        .layer(
            "layout",
            Subgraph::new("layouts:corrugation"),
            &[
                SurfaceOutput::Extra("profile".into()),
                SurfaceOutput::Extra("fixings".into()),
                SurfaceOutput::Extra("washers".into()),
            ],
        )
        .node(
            "tears",
            m(
                Add,
                m(
                    Mul,
                    Noise::perlin()
                        .periods(16, 2)
                        .octaves(5)
                        .persistence(0.6)
                        .seed(547),
                    0.7,
                ),
                m(
                    Mul,
                    Noise::perlin().periods(128, 16).octaves(2).seed(549),
                    0.3,
                ),
            ),
        )
        // Crests and troughs both, and not the flanks between.
        .node("exposed", Math::unary(Abs, m(Sub, "layout.profile", 0.5)))
        .node(
            "loss_bias",
            m(
                Add,
                m(
                    Add,
                    remap("tears", -0.7, 0.7),
                    // A little on crests and in troughs, less than the
                    // painted metal's own crest bias gives, so the tears
                    // follow the sheet without striping every corrugation.
                    m(
                        Sub,
                        m(Mul, "exposed", 0.2),
                        m(Mul, m(Sub, "layout.profile", 0.5), 0.25),
                    ),
                ),
                // Steep to the middle, then slower, so that even the oldest
                // sheet keeps streaks of paint.
                m(
                    Add,
                    m(Mul, m(Min, p("age"), 0.55), 0.9),
                    m(Sub, m(Mul, m(Max, m(Sub, p("age"), 0.55), 0.0), 0.5), 0.3),
                ),
            ),
        )
        .layer(
            "paint",
            Subgraph::new("library:painted-metal")
                .param("color", ParamValue::Color([0.38, 0.62, 0.72]))
                .param("wear", ParamValue::Float(0.12))
                .input("profile", "layout.profile")
                .input("loss_bias", "loss_bias"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("loss".into()),
            ],
        )
        .node("aged", Clamp::new(m(Mul, p("age"), 100.0)))
        .node(
            "seed",
            m(
                Mul,
                m(
                    Max,
                    "paint.loss",
                    m(
                        Mul,
                        "layout.washers",
                        Levels::new(p("age")).in_range(0.3, 0.6),
                    ),
                ),
                "aged",
            ),
        )
        .layer(
            "rust",
            Subgraph::new("weathering:rust")
                .input("base_color", "paint.base_color")
                .input("roughness", "paint.roughness")
                .input("metallic", "paint.metallic")
                .input("height", "paint.height")
                .input("seed_mask", "seed")
                .param("spread", ParamValue::Float(0.003))
                .param("pitting", ParamValue::Float(0.012))
                .param("blister", ParamValue::Float(0.006)),
            &rust_channels(),
        );
    let g = patch_coverage(g);
    oxide_finish(
        g,
        "paint",
        1.2,
        Oxide {
            holes: Some("layout.fixings"),
            run: 0.08,
        },
    )
}

fn rust_channels() -> Vec<SurfaceOutput> {
    vec![
        SurfaceOutput::Height,
        SurfaceOutput::Extra("rust_mask".into()),
    ]
}

/// Coverage for scale grown out of a seed by the rust compound's front: the
/// mask is opaque, thinned into lace at the margin, with loose specks round
/// it. Defines `coverage`, `core` and `haze` for [`oxide_finish`].
fn patch_coverage(g: MaterialGraphBuilder) -> MaterialGraphBuilder {
    use MathOp::{Add, Max, Mul};
    g.node("patch", Levels::new("rust.rust_mask").in_range(0.04, 0.3))
        .node("near", Blur::new("patch").radius(0.02))
        .node(
            "specks",
            m(
                Mul,
                Levels::new(Noise::perlin().period(128).octaves(2).seed(573)).in_range(0.6, 0.63),
                Levels::new("near").in_range(0.02, 0.3),
            ),
        )
        // Thin at the margin, where the substrate still shows through.
        .node(
            "lace",
            Clamp::new(m(
                Add,
                Levels::new(Noise::perlin().period(64).octaves(3).seed(575)).in_range(0.4, 0.43),
                Levels::new("near").in_range(0.45, 0.85),
            )),
        )
        .node("coverage", m(Max, m(Mul, "patch", "lace"), "specks"))
        .node("core", Levels::new("near").in_range(0.5, 1.0))
        .node("haze", m(Mul, Levels::new("near").in_range(0.0, 0.6), 0.18))
}

/// What differs between the callers of [`oxide_finish`].
struct Oxide<'a> {
    /// A mask that is a hole through the sheet, dark whatever it rusted to.
    holes: Option<&'a str>,
    /// How far a rust run bleeds down the sheet below the scale, in UV.
    run: f32,
}

// Colours, roughness and relief for scale on sheet metal, from three fields
// the caller defines: `coverage` (opaque scale), `core` (how old the scale
// is, fresh orange at zero to dark brown at one) and `haze` (a thin oxidation
// film over the bare substrate between). Every one of them must be exactly
// zero where nothing has rusted, and everything here is gated through `Mix`,
// so the caller's substrate then survives bit for bit. Runs bleed down the
// sheet from the scale.
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
fn oxide_finish(
    g: MaterialGraphBuilder,
    substrate: &str,
    tile: f32,
    oxide: Oxide<'_>,
) -> MaterialGraph {
    use MathOp::{Add, Max, Mul, Sub};
    let g = g
        .node(
            "oxide_grain",
            Noise::perlin().period(64).octaves(3).seed(557),
        )
        .node(
            "oxide_fleck",
            Levels::new(Noise::perlin().period(64).octaves(3).seed(561)).in_range(0.6, 0.75),
        )
        .node(
            "oxide_fine",
            Noise::perlin().period(256).octaves(2).seed(563),
        )
        .node(
            "oxide",
            Mix::new(
                Mix::new(
                    [0.33, 0.10, 0.022],
                    [0.09, 0.03, 0.01],
                    Clamp::new(m(
                        Add,
                        m(Add, m(Mul, "core", 0.6), m(Mul, "oxide_grain", 0.4)),
                        remap("oxide_fine", -0.3, 0.2),
                    )),
                ),
                [0.05, 0.02, 0.008],
                m(Mul, "oxide_fleck", 0.5),
            ),
        )
        // Runs: the scale smeared vertically and shifted down the sheet.
        .node(
            "run",
            Transform::new(Blur::directional("coverage", 90.0).radius(oxide.run))
                .translate(0.0, -oxide.run * 0.6),
        )
        .node(
            "stain",
            m(
                Mul,
                m(
                    Mul,
                    Levels::new("run").in_range(0.02, 0.5),
                    Levels::new(Noise::perlin().periods(64, 4).octaves(2).seed(571))
                        .in_range(0.3, 0.8),
                ),
                m(Mul, m(Sub, 1.0, "coverage"), 0.5),
            ),
        )
        .node(
            "filmed",
            Mix::new(
                format!("{substrate}.base_color"),
                m(Mul, format!("{substrate}.base_color"), [0.55, 0.45, 0.37]),
                m(Mul, "haze", m(Sub, 1.0, "coverage")),
            ),
        )
        .node("stained", Mix::new("filmed", [0.22, 0.08, 0.025], "stain"))
        .node("oxide_color", Mix::new("stained", "oxide", "coverage"))
        .node("bloom", m(Max, "stain", "haze"))
        .node(
            "oxide_roughness",
            Mix::new(
                Mix::new(format!("{substrate}.roughness"), 0.72, "bloom"),
                remap("oxide_grain", 0.8, 0.97),
                "coverage",
            ),
        )
        .node(
            "oxide_metallic",
            Mix::new(
                format!("{substrate}.metallic"),
                0.0,
                m(Max, "coverage", m(Mul, "bloom", 0.5)),
            ),
        )
        .node(
            "height",
            Mix::new(
                "rust.height",
                Clamp::new(m(
                    Add,
                    "rust.height",
                    m(
                        Mul,
                        m(
                            Add,
                            m(Add, m(Mul, "core", 0.4), m(Mul, "oxide_grain", 0.3)),
                            m(Mul, "oxide_fine", 0.3),
                        ),
                        0.035,
                    ),
                )),
                "coverage",
            ),
        );
    // A screw hole is a hole: dark whatever the sheet around it has become.
    let g = g.node(
        "surface_color",
        match oxide.holes {
            Some(mask) => Mix::new("oxide_color", [0.01, 0.01, 0.01], mask),
            None => Mix::new("oxide_color", "oxide_color", 0.0),
        },
    );
    finish(
        g,
        PbrOutput::new()
            .base_color("surface_color")
            .roughness("oxide_roughness")
            .metallic("oxide_metallic")
            .extra("seed", "seed"),
        tile,
        0.02,
    )
}
