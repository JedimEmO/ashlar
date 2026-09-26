//! The frontier-settlement surfaces: composite hull plating, diamond tread
//! plate, sand-coloured adobe and wind-rippled desert sand.
//!
//! Each is laid out in UV arithmetic or through the shared slab lattice, and
//! each writes the channels a live parameter moves as baked planes the
//! parameter only scales or thresholds, so a compiled twin with every
//! parameter live still binds a handful of images.
use super::surfaces::{finish, m, p, remap};
use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Clamp, Decompose, Invert, Levels, Math, Mix, Noise, Pattern, PatternKind, Scratches,
        Subgraph, Uv, Voronoi, VoronoiOutput, Warp,
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

fn lattice(columns: i32, rows: i32, offset: f32, joint: f32, bevel: f32) -> Subgraph {
    Subgraph::new("layouts:slab-lattice")
        .param("columns", ParamValue::Int(columns))
        .param("rows", ParamValue::Int(rows))
        .param("offset", ParamValue::Float(offset))
        .param("joint_width", ParamValue::Float(joint))
        .param("bevel", ParamValue::Float(bevel))
        .param("irregularity", ParamValue::Float(0.0))
}

/// `clamp((a - b) / width)`: zero at or below `b`, one from `b + width` up.
fn ramp(a: impl Into<Input>, b: impl Into<Input>, width: f32) -> Clamp {
    Clamp::new(m(MathOp::Div, m(MathOp::Sub, a, b), width))
}

/// Where a live `amount` in `0..=1` has spread over a `field` in `0..=1`: the
/// highest values of the field go first. A threshold rather than a multiply,
/// so a patch grows from its core rather than fading in everywhere at once.
fn spread(field: &str, amount: &str, softness: f32) -> Clamp {
    ramp(field, m(MathOp::Sub, 1.0, p(amount)), softness)
}

/// Composite hull plating: the prefab walls of a frontier settlement.
///
/// Three panel sizes on one bond. The base lattice lays metre-by-half-metre
/// panels in a running bond; a hash of each panel's identity subdivides some
/// into quarter-metre plates and splits others into half-metre squares, so
/// every seam of the small lattices lands on a seam of the large one. A few
/// panels are louvred grilles and a few are access hatches with an inner
/// groove. Fasteners run in a row a few centimetres inside every seam.
///
/// The paint is a semi-gloss composite, `color` tinted per panel. Grime sits
/// in the seams and hangs in streaks below each horizontal one (the kit lays
/// V up its walls). `wear` takes the paint off the arrises, the fastener
/// heads and scuffs first, down to bare metal. The height is a 12 mm range
/// over the two metre repeat and reads no parameter, so it bakes to one plane.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn hull_plating() -> MaterialGraph {
    use MathOp::{Abs, Add, Fract, Max, Min, Mul, Pow, Sqrt, Step, Sub};
    let g = MaterialGraph::builder("library:hull-plating")
        .param(Param::color("color", [0.56, 0.57, 0.56]))
        .param(Param::float("wear", 0.35).range(0.0, 1.0))
        .layer("big", lattice(2, 4, 0.5, 0.0032, 0.0012), &fields())
        .layer("medium", lattice(4, 4, 0.0, 0.0028, 0.0011), &fields())
        .layer("small", lattice(8, 8, 0.0, 0.0024, 0.001), &fields())
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        // What each large panel is: subdivided, split, a grille, a hatch or
        // plain, decided once by its own identity.
        .node("subdivided", m(Step, 0.2, "big.unit_id"))
        .node(
            "split",
            m(
                Mul,
                m(Step, "big.unit_id", 0.2),
                m(Step, 0.36, "big.unit_id"),
            ),
        )
        .node(
            "grille_panel",
            m(
                Mul,
                m(Step, "big.unit_id", 0.36),
                m(Step, 0.52, "big.unit_id"),
            ),
        )
        .node(
            "hatch_panel",
            m(
                Mul,
                m(Step, "big.unit_id", 0.52),
                m(Step, 0.62, "big.unit_id"),
            ),
        )
        .node(
            "face",
            m(
                Mul,
                "big.mask",
                m(
                    Mul,
                    Mix::new(1.0, "small.mask", "subdivided"),
                    Mix::new(1.0, "medium.mask", "split"),
                ),
            ),
        )
        .node(
            "edge",
            m(
                Min,
                "big.edge_distance",
                m(
                    Min,
                    Mix::new(1.0, "small.edge_distance", "subdivided"),
                    Mix::new(1.0, "medium.edge_distance", "split"),
                ),
            ),
        )
        .node(
            "panel_id",
            Mix::new(
                Mix::new("big.unit_id", "small.unit_id", "subdivided"),
                "medium.unit_id",
                "split",
            ),
        )
        .node(
            "top_v",
            Mix::new(
                Mix::new("big.local_v", "small.local_v", "subdivided"),
                "medium.local_v",
                "split",
            ),
        )
        // Fasteners: a 62.5 mm grid, kept where a cell's centre sits in the
        // band just inside a seam.
        .node(
            "rivet_u",
            m(Sub, Math::unary(Fract, m(Mul, "u", 32.0)), 0.5),
        )
        .node(
            "rivet_v",
            m(Sub, Math::unary(Fract, m(Mul, "v", 32.0)), 0.5),
        )
        .node(
            "rivet_r",
            Math::unary(
                Sqrt,
                m(
                    Add,
                    m(Mul, "rivet_u", "rivet_u"),
                    m(Mul, "rivet_v", "rivet_v"),
                ),
            ),
        )
        .node(
            "rivet_dome",
            Math::unary(Sqrt, Clamp::new(m(Sub, 1.0, m(Mul, "rivet_r", 5.2)))),
        )
        .node(
            "rivet_band",
            m(Mul, m(Step, 0.024, "edge"), m(Step, "edge", 0.006)),
        )
        .node("rivets", m(Mul, "rivet_dome", m(Mul, "rivet_band", "face")))
        // A grille: the panel's interior as a run of angled louvres.
        .node(
            "grille_zone",
            m(Mul, "grille_panel", ramp("edge", 0.03, 0.002)),
        )
        .node("louvre", Math::unary(Fract, m(Mul, "v", 128.0)))
        .node(
            "louvre_slope",
            m(Mul, m(Step, "louvre", 0.82), m(Mul, "louvre", 1.2)),
        )
        // A hatch: an inner groove six centimetres in.
        .node(
            "groove",
            m(
                Mul,
                "hatch_panel",
                Clamp::new(m(
                    Sub,
                    1.0,
                    m(Mul, Math::unary(Abs, m(Sub, "edge", 0.03)), 900.0),
                )),
            ),
        )
        .node("peel", Noise::perlin().period(128).octaves(2).seed(1201))
        .node("dent", Noise::perlin().period(8).octaves(3).seed(1203))
        .node(
            "face_height",
            m(
                Add,
                m(Add, 0.64, m(Mul, m(Sub, "panel_id", 0.5), 0.05)),
                m(
                    Add,
                    remap("peel", -0.006, 0.006),
                    remap("dent", -0.015, 0.015),
                ),
            ),
        )
        .node(
            "grilled",
            Mix::new(
                "face_height",
                m(Add, 0.36, m(Mul, "louvre_slope", 0.16)),
                "grille_zone",
            ),
        )
        .node(
            "plated",
            m(
                Add,
                m(Sub, "grilled", m(Mul, "groove", 0.22)),
                m(Mul, "rivets", 0.1),
            ),
        )
        .node("height", Mix::new(0.2, "plated", "face"))
        // Grime: in every seam, deep in a grille, and in streaks hanging from
        // the seam above.
        .node(
            "streak_field",
            Levels::new(Noise::perlin().periods(64, 4).octaves(3).seed(1207)).in_range(0.55, 0.85),
        )
        .node("below_seam", m(Pow, "top_v", 4.0))
        .node("soot", Noise::perlin().period(4).octaves(4).seed(1209))
        .node(
            "grime",
            Clamp::new(m(
                Add,
                m(
                    Add,
                    m(Mul, Invert::new("face"), 0.7),
                    m(Mul, "grille_zone", m(Sub, 1.0, "louvre_slope")),
                ),
                m(
                    Add,
                    m(
                        Mul,
                        m(Mul, "below_seam", "streak_field"),
                        remap("soot", 0.25, 0.75),
                    ),
                    m(Mul, "groove", 0.5),
                ),
            )),
        )
        // A replacement panel is a shade off the rest, warmer or cooler.
        .node(
            "replaced",
            m(Step, 0.9, Math::unary(Fract, m(Mul, "panel_id", 7.31))),
        )
        .node(
            "replacement_tint",
            Mix::new(
                [0.96, 0.985, 1.03],
                [1.04, 1.01, 0.95],
                m(Step, 0.5, Math::unary(Fract, m(Mul, "panel_id", 13.7))),
            ),
        )
        .node(
            "tint",
            Mix::new([1.0, 1.0, 1.0], "replacement_tint", "replaced"),
        )
        .node(
            "tone",
            m(
                Mul,
                m(
                    Mul,
                    remap("panel_id", 0.86, 1.07),
                    remap(Noise::perlin().period(16).octaves(3).seed(1211), 0.93, 1.04),
                ),
                m(Sub, 1.0, m(Mul, "grime", 0.6)),
            ),
        )
        // Livery: an accent band low on a few panels and a stencilled plate on
        // every hatch.
        .node(
            "accent_panel",
            m(
                Mul,
                m(Step, "big.unit_id", 0.62),
                m(Step, 0.72, "big.unit_id"),
            ),
        )
        .node(
            "accent",
            m(
                Mul,
                m(Mul, "accent_panel", "face"),
                m(
                    Mul,
                    m(Step, "big.local_v", 0.1),
                    m(Step, 0.16, "big.local_v"),
                ),
            ),
        )
        .node(
            "stencil",
            m(
                Mul,
                m(Mul, "hatch_panel", "face"),
                m(
                    Mul,
                    m(
                        Mul,
                        m(Step, "big.local_u", 0.1),
                        m(Step, 0.34, "big.local_u"),
                    ),
                    m(
                        Mul,
                        m(Step, "big.local_v", 0.8),
                        m(Step, 0.84, "big.local_v"),
                    ),
                ),
            ),
        )
        // Stencilled characters: dashes of paint with gaps between them.
        .node(
            "lettering",
            m(
                Mul,
                "stencil",
                m(
                    Mul,
                    m(Step, 0.3, Math::unary(Fract, m(Mul, "u", 96.0))),
                    m(Step, 0.2, Math::unary(Fract, m(Mul, "v", 256.0))),
                ),
            ),
        )
        // Where paint goes first: an arris, a fastener head, a knocked patch.
        .node(
            "wear_bias",
            Levels::new(Noise::perlin().period(8).octaves(4).seed(1217)).in_range(0.35, 0.8),
        )
        .node(
            "chip_grain",
            Levels::new(Noise::value().period(256).octaves(2).seed(1219)).in_range(0.3, 0.9),
        )
        .node(
            "wear_field",
            Clamp::new(m(
                Mul,
                m(
                    Max,
                    m(
                        Mul,
                        Clamp::new(m(Sub, 1.1, m(Mul, "edge", 70.0))),
                        m(Add, 0.45, m(Mul, "chip_grain", 0.75)),
                    ),
                    m(Mul, "rivets", 1.0),
                ),
                m(Mul, "face", m(Add, 0.62, m(Mul, "wear_bias", 0.6))),
            )),
        )
        .node("worn", spread("wear_field", "wear", 0.05))
        // Primer shows at the rim of a chip, metal in its heart.
        .node("bared", ramp("wear_field", m(Sub, 1.08, p("wear")), 0.04))
        .node(
            "scuffs",
            Scratches::new()
                .count(512)
                .length(0.025)
                .width(0.0008)
                .angle_spread(180.0)
                .seed(1213),
        )
        .node(
            "paint_roughness",
            m(
                Add,
                m(
                    Add,
                    m(Add, 0.36, remap("peel", -0.03, 0.03)),
                    m(Mul, "grime", 0.3),
                ),
                m(Mul, "scuffs", 0.2),
            ),
        )
        // The paint as two baked planes the live `color` only scales: what
        // the colour tints (the body paint, per-panel tint and tone), and what
        // it does not (the livery). A compiled twin with `color` and `wear`
        // live then binds these, the tone and the wear field, and no more.
        .node("finish_tone", m(Add, "tone", m(Mul, "scuffs", 0.06)))
        .node(
            "unpainted",
            m(Mul, Invert::new("lettering"), Invert::new("accent")),
        )
        .node("tinted", m(Mul, m(Mul, "tint", "unpainted"), "finish_tone"))
        .node(
            "livery",
            m(
                Mul,
                m(
                    Add,
                    m(
                        Mul,
                        [0.42, 0.14, 0.03],
                        m(Mul, "accent", Invert::new("lettering")),
                    ),
                    m(Mul, [0.07, 0.07, 0.075], "lettering"),
                ),
                "finish_tone",
            ),
        )
        .node("paint", m(Add, m(Mul, p("color"), "tinted"), "livery"))
        .node(
            "color_out",
            Mix::new(
                "paint",
                m(
                    Mul,
                    m(
                        Add,
                        [0.24, 0.25, 0.26],
                        m(Mul, [0.16, 0.155, 0.15], "bared"),
                    ),
                    "tone",
                ),
                "worn",
            ),
        )
        .node(
            "roughness",
            Mix::new("paint_roughness", Mix::new(0.6, 0.3, "bared"), "worn"),
        )
        .node("metallic", m(Mul, "worn", "bared"));
    finish(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic("metallic"),
        2.0,
        0.012,
    )
}

/// Diamond tread plate: the deck of a landing pad, a gantry or a ramp.
///
/// The lugs are the standard pattern, 31.25 mm apart on a metre repeat: a
/// rounded bar across each cell, turned a quarter one way on one colour of
/// a checker and the other way on the other, drawn as a signed distance to
/// its centre segment so the top is domed and the flank falls into the plate.
/// Feet polish the lug tops bright and smooth; grime and rust sit in the
/// recesses between them. `rust` spreads the oxide out of the recesses.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn tread_plate() -> MaterialGraph {
    use MathOp::{Abs, Add, Fract, Mul, Sqrt, Sub};
    let g = MaterialGraph::builder("library:tread-plate")
        .param(Param::float("rust", 0.25).range(0.0, 1.0))
        .layer(
            "steel",
            Subgraph::new("substances:steel").param("color", ParamValue::Color([0.2, 0.205, 0.21])),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Height,
            ],
        )
        .node("u", m(Mul, Decompose::new(Uv::new(), Channel::R), 32.0))
        .node("v", m(Mul, Decompose::new(Uv::new(), Channel::G), 32.0))
        .node("cu", m(Sub, Math::unary(Fract, "u"), 0.5))
        .node("cv", m(Sub, Math::unary(Fract, "v"), 0.5))
        .node("parity", Pattern::new(PatternKind::Checker).x(16).y(16))
        // Alternate cells lie along U and along V, so a lens's tips point
        // at its neighbours' flanks and never at their tips.
        .node("along", Mix::new("cu", "cv", "parity"))
        .node("across", Mix::new("cv", "cu", "parity"))
        // A lens: widest at its middle, pointed at both ends.
        .node(
            "taper",
            Clamp::new(m(Sub, 1.0, m(Mul, m(Mul, "along", "along"), 5.8))),
        )
        .node(
            "inside",
            m(Sub, m(Mul, "taper", 0.16), Math::unary(Abs, "across")),
        )
        .node("lug", Clamp::new(m(Mul, "inside", 16.0)))
        .node(
            "lug_profile",
            m(Mul, Math::unary(Sqrt, "lug"), Math::unary(Sqrt, "taper")),
        )
        .node("pits", Noise::value().period(256).seed(1301))
        .node(
            "height",
            m(
                Add,
                m(Add, 0.25, m(Mul, "lug_profile", 0.6)),
                m(
                    Add,
                    m(Mul, m(Sub, "steel.height", 0.5), 3.0),
                    remap("pits", -0.012, 0.0),
                ),
            ),
        )
        .node("recess", Invert::new(Clamp::new(m(Mul, "lug", 1.6))))
        .node(
            "polish",
            m(
                Mul,
                ramp("lug_profile", 0.72, 0.2),
                remap(Noise::perlin().period(16).octaves(2).seed(1303), 0.4, 1.0),
            ),
        )
        .node("dirt", Noise::perlin().period(8).octaves(4).seed(1307))
        .node(
            "grime",
            Clamp::new(m(Mul, "recess", remap("dirt", 0.25, 0.9))),
        )
        .node(
            "rust_field",
            Clamp::new(m(
                Add,
                m(
                    Mul,
                    m(Add, m(Mul, "recess", 0.55), 0.2),
                    Levels::new(Noise::perlin().period(8).octaves(4).seed(1309))
                        .in_range(0.35, 0.8),
                ),
                m(
                    Mul,
                    Levels::new(Noise::value().period(64).seed(1311)).in_range(0.8, 1.0),
                    0.35,
                ),
            )),
        )
        .node("rusted", spread("rust_field", "rust", 0.12))
        .node(
            "rust_color",
            Mix::new(
                [0.13, 0.045, 0.018],
                [0.3, 0.11, 0.035],
                Noise::perlin().period(64).octaves(2).seed(1313),
            ),
        )
        .node(
            "metal",
            m(
                Mul,
                Mix::new("steel.base_color", [0.58, 0.59, 0.6], m(Mul, "polish", 0.8)),
                m(Sub, 1.0, m(Mul, "grime", 0.55)),
            ),
        )
        .node("color", Mix::new("metal", "rust_color", "rusted"))
        .node(
            "metal_roughness",
            m(
                Add,
                Mix::new("steel.roughness", 0.2, "polish"),
                m(Mul, "grime", 0.35),
            ),
        )
        .node("roughness", Mix::new("metal_roughness", 0.85, "rusted"))
        .node("metallic", Mix::new(1.0, 0.0, "rusted"));
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic("metallic"),
        1.0,
        0.004,
    )
}

/// Sand-coloured adobe: a mud render smoothed by hand, for domed huts and
/// the walls round a compound.
///
/// Soft and undulating rather than thrown: broad swells where the render was
/// laid thick, rounded trowel sweeps inside them, and sand grains standing in
/// the surface. Some patches were redone and are a shade off and a touch
/// proud. Water has left soft darker runs down the wall (the kit lays V up
/// every wall, so the runs are stretched along V), and fine hairline cracks
/// meander through the drier patches. `color` is the clay.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn adobe() -> MaterialGraph {
    use MathOp::{Add, Div, Mul, Sqrt, Sub};
    let g = MaterialGraph::builder("library:adobe")
        .param(Param::color("color", [0.38, 0.26, 0.148]))
        .node("swell", Noise::perlin().period(2).octaves(4).seed(1401))
        .node("lumps", Noise::perlin().period(16).octaves(3).seed(1425))
        .node(
            "press_offset",
            remap(Noise::perlin().period(8).octaves(2).seed(1403), -0.04, 0.04),
        )
        // Hand-pressed pillows: each cell is where a palm smoothed the mud
        // outwards, highest in its middle and creased where two meet.
        .node(
            "presses",
            Warp::new(
                Voronoi::new()
                    .period(16)
                    .jitter(0.95)
                    .seed(1405)
                    .output(VoronoiOutput::Distance),
                "press_offset",
            )
            .amount(1.0),
        )
        .node(
            "pillow",
            Math::unary(Sqrt, Clamp::new(m(Sub, 1.0, m(Mul, "presses", "presses")))),
        )
        .node("grains", Noise::value().period(512).seed(1407))
        .node("grit", Noise::value().period(256).seed(1409))
        .node(
            "straw",
            Scratches::new()
                .count(512)
                .length(0.012)
                .width(0.0007)
                .angle_spread(180.0)
                .seed(1427),
        )
        .node(
            "patch_cells",
            Warp::new(
                Voronoi::new()
                    .period(8)
                    .jitter(0.8)
                    .seed(1411)
                    .output(VoronoiOutput::Cell),
                "press_offset",
            )
            .amount(1.2),
        )
        .node("patch", Levels::new("patch_cells").in_range(0.82, 0.84))
        .node(
            "meander",
            m(
                Add,
                remap(
                    Noise::perlin().period(16).octaves(2).seed(1413),
                    -0.02,
                    0.02,
                ),
                remap(
                    Noise::perlin().period(64).octaves(3).seed(1415),
                    -0.005,
                    0.005,
                ),
            ),
        )
        .node(
            "crack_cells",
            Warp::new(
                Voronoi::new()
                    .periods(4, 8)
                    .jitter(0.9)
                    .seed(1417)
                    .output(VoronoiOutput::Edge),
                "meander",
            )
            .amount(1.0),
        )
        .node(
            "crack_bias",
            Levels::new(Noise::perlin().period(4).octaves(2).seed(1419)).in_range(0.55, 0.68),
        )
        .node(
            "cracks",
            m(
                Mul,
                Clamp::new(m(Div, m(Sub, 0.005, "crack_cells"), 0.004)),
                m(Mul, "crack_bias", Invert::new("patch")),
            ),
        )
        .node(
            "height",
            m(
                Add,
                m(
                    Add,
                    remap("swell", 0.28, 0.52),
                    m(Add, m(Mul, "pillow", 0.2), remap("lumps", -0.09, 0.09)),
                ),
                m(
                    Sub,
                    m(
                        Add,
                        remap("grains", -0.015, 0.015),
                        m(Add, m(Mul, "patch", 0.04), m(Mul, "straw", 0.02)),
                    ),
                    m(Mul, "cracks", 0.07),
                ),
            ),
        )
        .node(
            "runs",
            Levels::new(Noise::perlin().periods(16, 2).octaves(3).seed(1421)).in_range(0.55, 0.85),
        )
        .node(
            "damp",
            m(
                Mul,
                "runs",
                Levels::new(Noise::perlin().periods(4, 2).octaves(3).seed(1423))
                    .in_range(0.4, 0.75),
            ),
        )
        .node(
            "batch",
            remap(Noise::perlin().period(4).octaves(3).seed(1429), 0.86, 1.1),
        )
        .node(
            "tone",
            m(
                Mul,
                m(
                    Mul,
                    m(Mul, "batch", remap("pillow", 0.94, 1.04)),
                    remap("grit", 0.88, 1.08),
                ),
                m(
                    Mul,
                    m(Sub, 1.0, m(Mul, "damp", 0.22)),
                    m(Sub, 1.0, m(Mul, "cracks", 0.5)),
                ),
            ),
        )
        .node(
            "patched",
            Mix::new(p("color"), m(Mul, p("color"), [1.1, 1.08, 1.03]), "patch"),
        )
        .node(
            "earth",
            Mix::new(
                m(Mul, "patched", "tone"),
                m(Mul, "patched", [0.62, 0.6, 0.56]),
                m(Mul, Levels::new("grains").in_range(0.85, 0.97), 0.7),
            ),
        )
        .node(
            "grain_color",
            Mix::new("earth", [0.46, 0.38, 0.2], m(Mul, "straw", 0.8)),
        )
        .node(
            "roughness",
            m(Add, remap("pillow", 0.92, 0.8), m(Mul, "damp", -0.06)),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("grain_color")
            .roughness("roughness")
            .metallic(0.0),
        2.0,
        0.035,
    )
}

/// Desert sand, wind-rippled: the ground a frontier settlement stands on.
///
/// Ripples run across V with a gentle windward face and a steep lee, their
/// crests bent by a broad noise so they fork and wander. Coarser, darker
/// patches of grit flatten them; small pebbles lie scattered on top. A 30 mm
/// range over a four metre repeat.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn desert_sand() -> MaterialGraph {
    use MathOp::{Add, Div, Fract, Min, Mul, Step, Sub};
    let g = MaterialGraph::builder("library:desert-sand")
        .param(Param::color("color", [0.55, 0.39, 0.22]))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("bend", Noise::perlin().period(4).octaves(3).seed(1501))
        .node("wobble", Noise::perlin().period(16).octaves(2).seed(1503))
        .node(
            "phase",
            m(
                Add,
                m(Mul, "v", 32.0),
                m(Add, m(Mul, "bend", 6.0), m(Mul, "wobble", 1.2)),
            ),
        )
        .node("f", Math::unary(Fract, "phase"))
        .node(
            "ripple_raw",
            m(Min, m(Div, "f", 0.64), m(Div, m(Sub, 1.0, "f"), 0.36)),
        )
        .node(
            "ripple",
            m(
                Mul,
                m(Mul, "ripple_raw", m(Sub, 2.0, "ripple_raw")),
                remap(Noise::perlin().period(8).octaves(2).seed(1513), 0.35, 1.0),
            ),
        )
        .node(
            "coarse",
            Levels::new(Noise::perlin().period(8).octaves(4).seed(1505)).in_range(0.55, 0.72),
        )
        .node("dune", Noise::perlin().period(1).octaves(3).seed(1507))
        .node("grain", Noise::value().period(512).seed(1509))
        .node(
            "stones",
            Voronoi::new()
                .period(128)
                .jitter(1.0)
                .seed(1511)
                .output(VoronoiOutput::Distance),
        )
        .node(
            "stone_cells",
            Voronoi::new()
                .period(128)
                .jitter(1.0)
                .seed(1511)
                .output(VoronoiOutput::Cell),
        )
        .node(
            "stone_here",
            m(
                Mul,
                m(Step, 0.035, "stone_cells"),
                Invert::new(m(Mul, "coarse", 0.5)),
            ),
        )
        .node(
            "pebble",
            m(
                Mul,
                "stone_here",
                Math::unary(
                    MathOp::Sqrt,
                    Clamp::new(m(
                        Div,
                        m(Sub, m(Add, 0.12, m(Mul, "stone_cells", 7.0)), "stones"),
                        0.2,
                    )),
                ),
            ),
        )
        .node(
            "height",
            m(
                Add,
                m(
                    Add,
                    remap("dune", 0.2, 0.42),
                    m(
                        Mul,
                        "ripple",
                        m(Mul, Invert::new(m(Mul, "coarse", 0.8)), 0.3),
                    ),
                ),
                m(Add, m(Mul, "pebble", 0.2), remap("grain", -0.015, 0.015)),
            ),
        )
        .node(
            "sand_tone",
            m(
                Mul,
                m(Mul, remap("grain", 0.9, 1.08), remap("dune", 0.94, 1.05)),
                m(
                    Sub,
                    1.0,
                    m(
                        Add,
                        m(Mul, "coarse", 0.22),
                        m(Mul, Invert::new("ripple"), 0.09),
                    ),
                ),
            ),
        )
        .node(
            "pebble_color",
            Mix::new(
                [0.2, 0.16, 0.12],
                [0.42, 0.33, 0.22],
                m(Mul, "stone_cells", 28.0),
            ),
        )
        .node(
            "base",
            Mix::new(
                m(Mul, p("color"), "sand_tone"),
                "pebble_color",
                Clamp::new(m(Mul, "pebble", 3.0)),
            ),
        )
        .node(
            "roughness",
            m(Add, remap("grain", 0.86, 0.96), m(Mul, "pebble", -0.12)),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("base")
            .roughness("roughness")
            .metallic(0.0),
        4.0,
        0.03,
    )
}
