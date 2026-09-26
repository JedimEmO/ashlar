//! Fired clay in lapped roofing courses or under an interior glaze.
use super::surfaces::{CHANNELS, finish, m, p, remap};
use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Clamp, Colorize, Decompose, GraphInput, Invert, Levels, Math, Mix, Noise, Subgraph, Uv,
        Voronoi, VoronoiOutput, Warp,
    },
};

fn fields() -> [SurfaceOutput; 6] {
    [
        "unit_id",
        "mask",
        "edge_distance",
        "joint_depth",
        "local_u",
        "local_v",
    ]
    .map(|s| SurfaceOutput::Extra(s.into()))
}

/// A stable hash in `0..1` of two whole-number ids.
fn hash(a: impl Into<Input>, b: impl Into<Input>, salt: f32) -> Math {
    use MathOp::{Add, Fract, Mul, Sin};
    Math::unary(
        Fract,
        m(
            Mul,
            Math::unary(
                Sin,
                m(
                    Add,
                    m(Add, m(Mul, a, 0.1731 + salt), m(Mul, b, 0.7549 - salt)),
                    0.391 + salt,
                ),
            ),
            4375.31,
        ),
    )
}

/// Kiln-fired terracotta: firing families, flashing, grog, pits and lime specks.
///
/// `unit_id` picks the unit's family, as the brick's colour families do: a
/// burnt minority, two ordinary bands and a pale, under-fired minority, each a
/// multiplier on `color` that `variety` scales towards one. Within a unit the
/// kiln flashes it darker in soft patches, grog makes a fine grain, sparse
/// pits sink into the skin and pale lime specks sit on it. Height is in the
/// library's 20 mm range around one half.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn fired_clay() -> MaterialGraph {
    use MathOp::{Add, Mul, Sub};
    let g = MaterialGraph::builder("substances:fired-clay")
        .param(Param::color("color", [0.50, 0.175, 0.088]))
        .param(Param::float("variety", 0.8).range(0.0, 1.0))
        .node("unit", GraphInput::float("unit_id", 0.5))
        .node(
            "family_table",
            Colorize::new("unit").gradient([
                (0.00, [0.60, 0.50, 0.48]),
                (0.16, [0.60, 0.50, 0.48]),
                (0.18, [0.84, 0.80, 0.80]),
                (0.52, [0.84, 0.80, 0.80]),
                (0.54, [1.00, 1.00, 1.00]),
                (0.86, [1.00, 1.00, 1.00]),
                (0.88, [1.14, 1.26, 1.34]),
                (1.00, [1.14, 1.26, 1.34]),
            ]),
        )
        .node("family", Mix::new(1.0, "family_table", p("variety")))
        // A second hash of the id, so a unit is a shade off its family.
        .node(
            "shade",
            remap(Math::unary(MathOp::Fract, m(Mul, "unit", 37.13)), 0.9, 1.08),
        )
        .node("firing", Noise::perlin().period(16).octaves(3).seed(701))
        .node("mottle", Noise::perlin().period(64).octaves(3).seed(703))
        .node("grog", Noise::perlin().period(256).octaves(2).seed(709))
        .node("fine", Noise::value().period(512).seed(711))
        .node(
            "pit_cells",
            Voronoi::new().period(128).seed(713).jitter(0.9),
        )
        .node(
            "pit_keep",
            Levels::new(
                Voronoi::new()
                    .period(128)
                    .seed(713)
                    .jitter(0.9)
                    .output(VoronoiOutput::Cell),
            )
            .in_range(0.93, 0.94),
        )
        .node(
            "pit",
            m(
                Mul,
                Invert::new(Levels::new("pit_cells").in_range(0.02, 0.12)),
                "pit_keep",
            ),
        )
        .node(
            "speck_cells",
            Voronoi::new().period(256).seed(719).jitter(0.9),
        )
        .node(
            "speck_keep",
            Levels::new(
                Voronoi::new()
                    .period(256)
                    .seed(719)
                    .jitter(0.9)
                    .output(VoronoiOutput::Cell),
            )
            .in_range(0.9, 0.91),
        )
        .node(
            "speck",
            m(
                Mul,
                Invert::new(Levels::new("speck_cells").in_range(0.04, 0.16)),
                "speck_keep",
            ),
        )
        .node(
            "flash",
            Levels::new("firing")
                .in_range(0.3, 0.75)
                .out_range(0.8, 1.06),
        )
        .node(
            "clay",
            m(
                Mul,
                m(Mul, p("color"), m(Mul, "family", "shade")),
                m(
                    Mul,
                    "flash",
                    m(Mul, remap("grog", 0.86, 1.08), remap("fine", 0.93, 1.04)),
                ),
            ),
        )
        // Mottling moves the hue a little, towards a yellower, drier clay.
        .node(
            "mottled",
            Mix::new(
                "clay",
                m(Mul, "clay", [1.1, 1.18, 1.2]),
                Levels::new("mottle").in_range(0.45, 0.8),
            ),
        )
        .node(
            "pitted",
            Mix::new("mottled", m(Mul, "mottled", 0.6), m(Mul, "pit", 0.6)),
        )
        .node(
            "color",
            Mix::new("pitted", [0.42, 0.36, 0.28], m(Mul, "speck", 0.7)),
        )
        .node(
            "roughness",
            m(
                Add,
                remap("grog", 0.74, 0.86),
                m(Add, m(Mul, "pit", 0.08), m(Mul, "speck", -0.06)),
            ),
        )
        .node(
            "height",
            m(
                Sub,
                m(
                    Add,
                    remap("firing", 0.49, 0.51),
                    m(
                        Add,
                        remap("grog", -0.03, 0.03),
                        remap("fine", -0.008, 0.008),
                    ),
                ),
                m(Sub, m(Mul, "pit", 0.12), m(Mul, "speck", 0.015)),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0),
        2.9,
        0.02,
    )
}

/// Barrel tiles laid in lapped courses, aligned down the slope.
///
/// Each unit is one tile's exposed length: a circular barrel across its
/// width that narrows towards its head, rising along the course to a rounded
/// lip that overhangs the course below. `irregularity` shifts each course
/// sideways, each tile off its column and each lip up the slope, so no two
/// laps agree. Where no tile covers the cell — the gaps between barrels and
/// the strip under a short lip — the profile drops to the shadowed deck.
///
/// Extras: `profile` (height in `0..=1` of the layout's own relief range),
/// `unit_id`, `mask` (tile coverage), `edge_distance` (UV), `joint_depth`,
/// `local_u`, `local_v` (down the course, zero at the head) and `arch`
/// (the barrel's crest, one on the ridge and zero at its sides) and
/// `lip_band` (one at the lip, fading to zero over the last fifth of the
/// exposed length).
#[must_use]
#[expect(clippy::too_many_lines, reason = "the layout's one shared partition")]
pub fn lapped_courses() -> MaterialGraph {
    use MathOp::{Abs, Add, Div, Floor, Fract, Min, Mul, Sqrt, Sub};
    MaterialGraph::builder("layouts:lapped-courses")
        .param(Param::int("columns", 16).range(1.0, 64.0))
        .param(Param::int("rows", 12).range(1.0, 64.0))
        .param(Param::float("taper", 0.035).range(0.0, 0.2))
        .param(Param::float("irregularity", 0.6).range(0.0, 1.0))
        .node(
            "columns",
            Math::unary(Floor, Clamp::new(p("columns")).range(1.0, 64.0)),
        )
        .node(
            "rows",
            Math::unary(Floor, Clamp::new(p("rows")).range(1.0, 64.0)),
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
        .node("local_v", Math::unary(Fract, "row"))
        .node("course_shift", hash("row_id", 3.0, 0.013))
        .node(
            "across",
            m(
                Add,
                m(Mul, "u", "columns"),
                m(
                    Mul,
                    m(Sub, "course_shift", 0.5),
                    m(Mul, p("irregularity"), 0.3),
                ),
            ),
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
        .node("unit_id", hash("column_id", "row_id", 0.0))
        .node("sway", hash("column_id", "row_id", 0.031))
        .node("reach", hash("column_id", "row_id", 0.057))
        .node(
            "centre",
            m(
                Add,
                0.5,
                m(Mul, m(Sub, "sway", 0.5), m(Mul, p("irregularity"), 0.07)),
            ),
        )
        .node(
            "half_width",
            m(Sub, 0.5, m(Mul, p("taper"), Invert::new("local_v"))),
        )
        .node("x", m(Div, m(Sub, "local_u", "centre"), "half_width"))
        .node(
            "arch",
            Math::unary(Sqrt, Clamp::new(Invert::new(m(Mul, "x", "x")))),
        )
        // Coverage across the barrel, softened over the clay's thickness.
        .node(
            "side",
            Clamp::new(m(
                Div,
                Invert::new(Math::unary(Abs, "x")),
                m(Div, 0.035, "half_width"),
            )),
        )
        .node(
            "lip",
            m(Sub, 0.985, m(Mul, "reach", m(Mul, p("irregularity"), 0.13))),
        )
        .node("along", Clamp::new(m(Div, m(Sub, "lip", "local_v"), 0.05)))
        .node("mask", m(Mul, "side", "along"))
        .node(
            "lip_band",
            Invert::new(Clamp::new(m(Div, m(Sub, "lip", "local_v"), 0.2))),
        )
        .node("joint_depth", Invert::new("mask"))
        .node(
            "edge_distance",
            m(
                Min,
                m(
                    Div,
                    m(Mul, Invert::new(Math::unary(Abs, "x")), "half_width"),
                    "columns",
                ),
                m(Div, m(Sub, "lip", "local_v"), "rows"),
            ),
        )
        // The clay rises along the course to the lip and bulges over the
        // barrel; the lip and the sides roll off over the clay's thickness.
        .node(
            "tile",
            m(
                Add,
                m(Add, 0.2, m(Mul, "arch", 0.56)),
                m(Mul, "local_v", 0.14),
            ),
        )
        .node("deck", m(Add, 0.06, m(Mul, "local_v", 0.08)))
        .node(
            "roll",
            m(Mul, Math::unary(Sqrt, "side"), Math::unary(Sqrt, "along")),
        )
        .node("profile", Mix::new("deck", "tile", "roll"))
        .tile_metres([2.9; 2])
        .output(
            PbrOutput::new()
                .extra("profile", "profile")
                .extra("unit_id", "unit_id")
                .extra("mask", "mask")
                .extra("edge_distance", "edge_distance")
                .extra("joint_depth", "joint_depth")
                .extra("local_u", "local_u")
                .extra("local_v", "local_v")
                .extra("arch", "arch")
                .extra("lip_band", "lip_band"),
        )
        .into_graph()
}

/// Weathered terracotta barrel tiles over a 2.9 m repeat, 100 mm of relief.
///
/// Sixteen tiles across and twelve courses, each tile a unit of
/// `substances:fired-clay`, pushed further per tile towards a deep red-brown
/// minority and a few bleached tiles. Weather follows the relief: soot and
/// grime settle in the troughs between barrels and at each tile's head under
/// the lip above, the lip darkens where water drips, the crown bleaches, and
/// lichen grows in a few sparse colonies of soft rosettes on the crowns and
/// wet lips while most tiles stay clean. The gaps are the shadowed course
/// beneath. `weathering` scales all of it together.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn clay_roof_tiles() -> MaterialGraph {
    use MathOp::{Add, Div, Mul, Sub};
    let g = MaterialGraph::builder("library:clay-roof-tiles")
        .param(Param::float("weathering", 0.5).range(0.0, 1.0))
        .layer(
            "layout",
            Subgraph::new("layouts:lapped-courses"),
            &[
                SurfaceOutput::Extra("profile".into()),
                SurfaceOutput::Extra("unit_id".into()),
                SurfaceOutput::Extra("mask".into()),
                SurfaceOutput::Extra("local_v".into()),
                SurfaceOutput::Extra("arch".into()),
                SurfaceOutput::Extra("lip_band".into()),
                SurfaceOutput::Extra("edge_distance".into()),
            ],
        )
        .layer(
            "clay",
            Subgraph::new("substances:fired-clay")
                .input("unit_id", "layout.unit_id")
                .param("color", ParamValue::Color([0.53, 0.16, 0.072])),
            &CHANNELS,
        )
        // Per tile, beyond the clay's own families: a deep red-brown
        // minority and a few sun-bleached tiles, and a mottle across each.
        .node(
            "deep",
            Levels::new(Math::unary(MathOp::Fract, m(Mul, "layout.unit_id", 53.71)))
                .in_range(0.7, 0.74),
        )
        .node(
            "pale",
            Levels::new(Math::unary(MathOp::Fract, m(Mul, "layout.unit_id", 91.37)))
                .in_range(0.86, 0.9),
        )
        .node("mottle", Noise::perlin().period(32).octaves(3).seed(741))
        .node(
            "toned",
            m(
                Mul,
                Mix::new(
                    Mix::new(
                        "clay.base_color",
                        m(Mul, "clay.base_color", [0.6, 0.44, 0.4]),
                        "deep",
                    ),
                    m(Mul, "clay.base_color", [1.2, 1.32, 1.4]),
                    m(Mul, "pale", 0.8),
                ),
                Mix::new(
                    [0.82, 0.76, 0.74],
                    [1.08, 1.1, 1.1],
                    Levels::new("mottle").in_range(0.25, 0.75),
                ),
            ),
        )
        // Soot and grime: in the troughs down each barrel's flanks and at a
        // tile's head, in the shadow of the lip above; streaked downslope.
        .node(
            "streak",
            Noise::perlin().periods(64, 8).octaves(3).seed(731),
        )
        .node(
            "trough",
            Invert::new(Levels::new("layout.arch").in_range(0.0, 0.85)),
        )
        .node(
            "head",
            Invert::new(Levels::new("layout.local_v").in_range(0.0, 0.3)),
        )
        .node(
            "grime",
            m(
                Mul,
                m(Add, 0.3, m(Mul, p("weathering"), 1.4)),
                m(
                    Mul,
                    Clamp::new(m(
                        Add,
                        m(Mul, "trough", "trough"),
                        m(Mul, m(Mul, "head", "head"), 0.8),
                    )),
                    remap("streak", 0.5, 1.0),
                ),
            ),
        )
        .node(
            "bleach",
            m(
                Mul,
                m(Mul, "layout.arch", "layout.mask"),
                m(Mul, p("weathering"), 0.2),
            ),
        )
        .node(
            "sunned",
            Mix::new("toned", m(Mul, "toned", [1.1, 1.16, 1.2]), "bleach"),
        )
        .node(
            "grimed",
            Mix::new(
                "sunned",
                m(Mul, "sunned", [0.3, 0.26, 0.24]),
                Clamp::new(m(Mul, "grime", 0.85)),
            ),
        )
        // Rain stains run down each tile in thin, broken streaks.
        .node(
            "runs",
            Noise::perlin().periods(128, 16).octaves(3).seed(737),
        )
        .node(
            "stained",
            Mix::new(
                "grimed",
                m(Mul, "grimed", [0.74, 0.66, 0.6]),
                m(
                    Mul,
                    Levels::new("runs").in_range(0.55, 0.8),
                    m(Mul, p("weathering"), 1.0),
                ),
            ),
        )
        // Water drips off the lip, so the clay darkens towards it.
        .node(
            "dripped",
            Mix::new(
                "stained",
                m(Mul, "stained", [0.66, 0.58, 0.54]),
                m(
                    Mul,
                    m(Mul, "layout.lip_band", "layout.lip_band"),
                    m(Add, 0.4, m(Mul, p("weathering"), 0.6)),
                ),
            ),
        )
        // Lichen: a few colonies, each a handful of soft rosettes. A coarse
        // field decides where colonies are, so most tiles stay clean; growth
        // favours the crown and the wet lip; the edge is frayed by noise.
        .node("colony", Noise::perlin().period(8).octaves(2).seed(721))
        .node("wobble", Noise::perlin().period(64).octaves(2).seed(723))
        .node("fray", Noise::perlin().period(256).octaves(1).seed(729))
        .node(
            "rosette_cells",
            Warp::new(Voronoi::new().period(32).seed(727).jitter(0.9), "wobble").amount(0.006),
        )
        .node(
            "rosette_id",
            Warp::new(
                Voronoi::new()
                    .period(32)
                    .seed(727)
                    .jitter(0.9)
                    .output(VoronoiOutput::Cell),
                "wobble",
            )
            .amount(0.006),
        )
        .node(
            "rosette",
            m(
                Mul,
                Clamp::new(Invert::new(m(
                    Div,
                    "rosette_cells",
                    remap(
                        Math::unary(MathOp::Fract, m(Mul, "rosette_id", 7.31)),
                        0.18,
                        0.5,
                    ),
                ))),
                Levels::new("rosette_id").in_range(0.35, 0.4),
            ),
        )
        .node(
            "habitat",
            m(
                Mul,
                m(
                    Mul,
                    Levels::new("colony").in_range(0.56, 0.72),
                    m(Add, 0.2, m(Mul, p("weathering"), 1.6)),
                ),
                m(
                    Mul,
                    "layout.mask",
                    m(
                        Add,
                        remap("layout.arch", 0.35, 1.0),
                        m(Mul, "layout.lip_band", 0.5),
                    ),
                ),
            ),
        )
        .node(
            "lichen",
            m(
                Mul,
                Levels::new(m(
                    Add,
                    m(Mul, "rosette", "habitat"),
                    remap("fray", -0.12, 0.12),
                ))
                .in_range(0.12, 0.6),
                "layout.mask",
            ),
        )
        .node(
            "lichen_color",
            Mix::new([0.26, 0.25, 0.11], [0.33, 0.31, 0.16], "wobble"),
        )
        .node(
            "crusted",
            Mix::new("dripped", "lichen_color", m(Mul, "lichen", 0.8)),
        )
        // The deck in the gaps and under a short lip is a recess: the clay
        // of the course beneath, in shadow, darkest where it is deepest.
        .node(
            "recess",
            Clamp::new(m(Div, m(Mul, "layout.edge_distance", -1.0), 0.004)),
        )
        .node(
            "deck_color",
            m(
                Mul,
                m(Mul, "clay.base_color", [0.3, 0.25, 0.23]),
                m(Mul, remap("recess", 1.0, 0.45), Mix::new(1.0, "ao", 0.8)),
            ),
        )
        .node("color", Mix::new("deck_color", "crusted", "layout.mask"))
        .node(
            "roughness",
            Mix::new(Mix::new("clay.roughness", 0.92, "grime"), 0.95, "lichen"),
        )
        .node(
            "height",
            m(
                Add,
                "layout.profile",
                m(
                    Mul,
                    "layout.mask",
                    m(
                        Add,
                        m(Div, m(Sub, "clay.height", 0.5), 6.0),
                        m(Mul, "lichen", 0.008),
                    ),
                ),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0),
        2.9,
        0.1,
    )
}

/// White glazed 25 cm ceramic tiles with 2 mm cement joints, in a metre repeat.
///
/// The glaze is what the eye reads, so it carries the detail: each tile sits
/// a fraction of a millimetre out of plane (lippage) so reflections break at
/// every joint, its face has a faint long-wave ripple and a finer orange peel,
/// and its edge rolls into a cushion that catches a highlight. The grout is
/// cast cement a few millimetres below the faces, greyed by patchy grime.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn ceramic_tile() -> MaterialGraph {
    use MathOp::{Add, Div, Mul, Sub};
    let g = MaterialGraph::builder("library:ceramic-tile")
        .param(Param::color("color", [0.72, 0.715, 0.69]))
        .param(Param::float("glaze", 1.0).range(0.0, 1.0))
        .param(Param::float("grime", 0.4).range(0.0, 1.0))
        .layer(
            "layout",
            Subgraph::new("layouts:slab-lattice")
                .param("joint_width", ParamValue::Float(0.002))
                .param("bevel", ParamValue::Float(0.0015))
                .param("irregularity", ParamValue::Float(0.0)),
            &fields(),
        )
        .layer(
            "clay",
            Subgraph::new("substances:fired-clay").input("unit_id", "layout.unit_id"),
            &CHANNELS,
        )
        .layer(
            "grout",
            Subgraph::new("substances:cast-cement")
                .param("color", ParamValue::Color([0.23, 0.235, 0.2]))
                .param("casting", ParamValue::Float(0.0)),
            &CHANNELS,
        )
        .node("orange_peel", Noise::perlin().period(128).seed(727))
        .node("ripple", Noise::perlin().period(16).octaves(2).seed(729))
        .node(
            "grime_noise",
            Noise::perlin().period(64).octaves(3).seed(733),
        )
        // Two more hashes of the tile's id tilt it about each axis.
        .node(
            "tilt_u",
            m(
                Sub,
                Math::unary(MathOp::Fract, m(Mul, "layout.unit_id", 17.31)),
                0.5,
            ),
        )
        .node(
            "tilt_v",
            m(
                Sub,
                Math::unary(MathOp::Fract, m(Mul, "layout.unit_id", 71.93)),
                0.5,
            ),
        )
        .node(
            "lippage",
            m(
                Add,
                m(Mul, "tilt_u", m(Sub, "layout.local_u", 0.5)),
                m(Mul, "tilt_v", m(Sub, "layout.local_v", 0.5)),
            ),
        )
        // The cushion: the glaze rolls off over the last 4 mm to the joint.
        .node(
            "cushion",
            Invert::new(Clamp::new(m(Div, "layout.edge_distance", 0.004))),
        )
        // Colour and roughness are written as baked planes that the three
        // parameters only scale and sum, so a compiled tile with `color`,
        // `glaze` and `grime` live binds three colour planes and one packed
        // scalar image rather than every input of every mix. The arithmetic is
        // the mix it replaces: grout, grime over the grout, the clay face
        // fading out under the glaze, and the glaze colour fading in.
        .node("grout_mask", Invert::new("layout.mask"))
        .node("grime_noise_level", remap("grime_noise", 0.1, 0.8))
        .node("grout_base", m(Mul, "grout.base_color", "grout_mask"))
        .node(
            "grout_grime",
            m(
                Mul,
                m(Mul, "grout_base", "grime_noise_level"),
                [-0.4, -0.43, -0.48],
            ),
        )
        .node("clay_face", m(Mul, "clay.base_color", "layout.mask"))
        .node(
            "glaze_tone",
            m(Mul, remap("layout.unit_id", 0.965, 1.02), "layout.mask"),
        )
        .node(
            "color",
            m(
                Add,
                m(Add, "grout_base", m(Mul, p("grime"), "grout_grime")),
                m(
                    Add,
                    m(Mul, m(Sub, 1.0, p("glaze")), "clay_face"),
                    m(Mul, m(Mul, p("glaze"), p("color")), "glaze_tone"),
                ),
            ),
        )
        .node(
            "unglazed_roughness",
            Mix::new("grout.roughness", "clay.roughness", "layout.mask"),
        )
        .node(
            "grime_roughness",
            m(
                Mul,
                m(Mul, "grout_mask", m(Mul, "grime_noise_level", 0.5)),
                m(Sub, 0.95, "grout.roughness"),
            ),
        )
        .node(
            "glaze_roughness",
            m(
                Mul,
                "layout.mask",
                m(
                    Sub,
                    m(
                        Add,
                        remap("orange_peel", 0.06, 0.12),
                        m(Mul, "cushion", 0.04),
                    ),
                    "clay.roughness",
                ),
            ),
        )
        .node(
            "roughness",
            m(
                Add,
                "unglazed_roughness",
                m(
                    Add,
                    m(Mul, p("grime"), "grime_roughness"),
                    m(Mul, p("glaze"), "glaze_roughness"),
                ),
            ),
        )
        .node(
            "glaze_relief",
            m(
                Add,
                m(Add, m(Mul, "lippage", 0.12), remap("ripple", -0.01, 0.01)),
                m(
                    Sub,
                    remap("orange_peel", -0.001, 0.001),
                    m(Mul, m(Mul, "cushion", "cushion"), 0.12),
                ),
            ),
        )
        .node(
            "face",
            m(
                Add,
                0.65,
                // The relief does not read `glaze`: a glaze thin enough to
                // follow the body leaves a trace of it, and a height free of
                // live parameters bakes to one plane when `glaze` is live.
                m(
                    Add,
                    m(Mul, m(Sub, "clay.height", 0.5), 0.12),
                    "glaze_relief",
                ),
            ),
        )
        .node(
            "height",
            Mix::new(m(Mul, "grout.height", 0.8), "face", "layout.mask"),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0),
        1.0,
        0.02,
    )
}
