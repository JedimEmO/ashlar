//! Boards share one partition and one timber substance, with an optional coat.
//!
//! The board lattice lays courses of boards with random butt joints and hands
//! each board its own coordinate frame. The timber substance draws the grain
//! in that frame, as a log cut by a plane: every board has its own pith, depth
//! and taper, so ring lines run along the board, arch into cathedrals where
//! the cut crosses them and bend round knots, and no two boards agree.
use super::surfaces::{finish, m, p, remap};
use crate::MaterialGraphBuilder;
use crate::{
    Channel, Input, MaterialGraph, MathOp, Param, ParamValue, PbrOutput, SurfaceOutput,
    nodes::{
        Clamp, Decompose, GraphInput, Invert, Levels, Math, Mix, Noise, OcclusionFromHeight,
        Scratches, Subgraph, Uv,
    },
};

/// The layout fields a board material reads.
const FIELDS: [&str; 8] = [
    "unit_id",
    "mask",
    "edge_distance",
    "joint_depth",
    "local_u",
    "local_v",
    "length",
    "end_mask",
];

fn fields() -> [SurfaceOutput; 8] {
    FIELDS.map(|s| SurfaceOutput::Extra(s.into()))
}

/// The timber channels and the grain fields a board material reads.
fn grain() -> [SurfaceOutput; 8] {
    [
        SurfaceOutput::BaseColor,
        SurfaceOutput::Roughness,
        SurfaceOutput::Metallic,
        SurfaceOutput::Height,
        SurfaceOutput::Extra("knots".into()),
        SurfaceOutput::Extra("knot_rims".into()),
        SurfaceOutput::Extra("late".into()),
        SurfaceOutput::Extra("fibre".into()),
    ]
}

/// Timber in the frame of the board the layout named `layout` drew.
fn timber_on_boards(layout: &str) -> Subgraph {
    Subgraph::new("substances:timber")
        .input("unit_id", format!("{layout}.unit_id"))
        .input("local_u", format!("{layout}.local_u"))
        .input("local_v", format!("{layout}.local_v"))
        .input("length", format!("{layout}.length"))
}

/// A fresh value in `0..1` from a stable one, by the lattice's sine hash.
fn hash(x: impl Into<Input>, scale: f32, bias: f32) -> Math {
    Math::unary(
        MathOp::Fract,
        m(
            MathOp::Mul,
            Math::unary(MathOp::Sin, m(MathOp::Add, m(MathOp::Mul, x, scale), bias)),
            4375.31,
        ),
    )
}

fn ramp(input: impl Into<Input>, low: f32, high: f32) -> Levels {
    Levels::new(input).in_range(low, high)
}

/// Courses of boards with random butt joints, eight courses over a 1.3 m repeat.
///
/// Every course is cut twice across the repeat, at a place and a split hashed
/// from the course, so boards run from 0.3 to 0.7 of the repeat when `split` is
/// one; a course is one board with a single butt joint when it is zero. Joints
/// close at the repeat seam because the cut positions are taken modulo one.
///
/// Extras: `unit_id` per board; `local_u` along the board and `local_v`
/// across it, both `0..1`; `length`, the board's length in UV; `mask`, the
/// board face inside its joints and bevel; `end_mask`, the same for the butt
/// joints alone, for a lapped course whose long edges are not joints;
/// `edge_distance` in UV and `joint_depth`, the complement of `mask`.
#[must_use]
pub fn board_lattice() -> MaterialGraph {
    use MathOp::{Add, Div, Floor, Fract, Min, Mul, Step, Sub};
    MaterialGraph::builder("layouts:board-lattice")
        .param(Param::int("rows", 8).range(1.0, 64.0))
        .param(Param::float("split", 1.0).range(0.0, 1.0))
        .param(Param::float("joint_width", 0.0012).range(0.0, 0.03))
        .param(Param::float("bevel", 0.0008).range(0.0002, 0.01))
        .param(Param::float("irregularity", 0.3).range(0.0, 1.0))
        .node(
            "rows",
            Math::unary(Floor, Clamp::new(p("rows")).range(1.0, 64.0)),
        )
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("row", m(Mul, "v", "rows"))
        .node("row_id", Math::unary(Floor, "row"))
        .node("local_v", Math::unary(Fract, "row"))
        .node("shift", hash("row_id", 0.1731, 0.391))
        // One board a course when `split` is zero, otherwise a cut between
        // three and seven tenths of the course.
        .node(
            "cut",
            Mix::new(
                1.0,
                m(Add, 0.3, m(Mul, hash("row_id", 0.7131, 0.113), 0.4)),
                Clamp::new(p("split")),
            ),
        )
        .node("along", Math::unary(Fract, m(Sub, "u", "shift")))
        .node("second", Math::new(Step, "along", "cut"))
        .node("length", Mix::new("cut", m(Sub, 1.0, "cut"), "second"))
        .node("from_start", m(Sub, "along", m(Mul, "second", "cut")))
        .node("local_u", m(Div, "from_start", "length"))
        .node("du", m(Min, "from_start", m(Sub, "length", "from_start")))
        .node(
            "dv",
            m(Div, m(Min, "local_v", Invert::new("local_v")), "rows"),
        )
        .node("edge_distance", m(Min, "du", "dv"))
        .node(
            "unit_id",
            hash(m(Add, m(Mul, "row_id", 2.0), "second"), 0.3713, 0.2271),
        )
        .node("edge_grain", Noise::perlin().period(128).seed(71))
        .node(
            "chips",
            m(
                Mul,
                p("irregularity"),
                m(
                    Add,
                    Levels::new("edge_grain").out_range(0.0, 0.0008),
                    m(Mul, "unit_id", 0.0004),
                ),
            ),
        )
        .node("gap", m(Add, m(Mul, p("joint_width"), 0.5), "chips"))
        .node(
            "mask",
            Clamp::new(m(Div, m(Sub, "edge_distance", "gap"), p("bevel"))),
        )
        .node(
            "end_mask",
            Clamp::new(m(Div, m(Sub, "du", "gap"), p("bevel"))),
        )
        .node("joint_depth", Invert::new("mask"))
        .output(
            FIELDS
                .iter()
                .fold(PbrOutput::new(), |out, name| out.extra(*name, *name)),
        )
        .into_graph()
}

/// A knot slot's frame: its offset from the board point in metres, along the
/// board (`kx_{slot}`) and across it (`ky_{slot}`), and the oval distance
/// (`knot_{slot}`), drawn out along the board as a flat-sawn knot is. The
/// centre is hashed from the board and the slot's salt.
fn knot_slot(g: MaterialGraphBuilder, slot: &str, salt: f32, bias: f32) -> MaterialGraphBuilder {
    use MathOp::{Add, Div, Mul, Sqrt, Sub};
    let centre_u = m(Add, 0.1, m(Mul, hash("unit", salt, bias), 0.8));
    let centre_v = m(
        Add,
        0.18,
        m(Mul, hash("unit", salt * 1.37, bias + 0.21), 0.64),
    );
    let kx = format!("kx_{slot}");
    let ky = format!("ky_{slot}");
    g.node(&kx, m(Mul, m(Sub, "lu", centre_u), "board_len"))
        .node(&ky, m(Mul, m(Sub, "lv", centre_v), p("board_width")))
        .node(
            format!("knot_{slot}"),
            Math::unary(
                Sqrt,
                m(
                    Add,
                    m(Mul, m(Div, &kx, 2.1), m(Div, &kx, 2.1)),
                    m(Mul, &ky, &ky),
                ),
            ),
        )
}

/// Timber cut from a log: per-board rings, cathedrals, knots, pores and fibre.
///
/// The board frame comes from the caller's layout: `local_u` along the board,
/// `local_v` across it and `length` in UV. The rings are the distance from a
/// per-board pith, whose offset, depth and taper are hashed from `unit_id`, so
/// a board near the pith shows cathedral arches and one far from it straight
/// lines. Three scales of noise bend that distance and a fourth moves the
/// ring phase, so rings wander, pinch together and spread; the late-wood
/// contrast fades in and out along the board and breaks into streaks. Three
/// knot slots a board — a large knot, a small one and a pin knot — bend the
/// rings round an oval core with a dark rim, and a large knot may carry a
/// check along the grain. Height is in the 20 mm range: late wood stands
/// proud of the early by a quarter millimetre, as on a brushed board.
///
/// Fine streaks are laid no finer than 256 cells across the repeat, so a
/// 512 bake still has two texels a cell and does not alias them into dashes.
///
/// Extras: `knots` (the knot cores), `knot_rims` (the dark ring round
/// them), `late` (the late-wood field) and `fibre` (the fine along-grain
/// streaking).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn timber() -> MaterialGraph {
    use MathOp::{Abs, Add, Div, Exp2, Fract, Mul, Pow, Sin, Sqrt, Step, Sub};
    let g = MaterialGraph::builder("substances:timber")
        .param(Param::float("repeat", 1.3).range(0.1, 8.0))
        .param(Param::float("board_width", 0.1625).range(0.02, 1.0))
        .param(Param::color("early_color", [0.2, 0.1, 0.027]))
        .param(Param::color("late_color", [0.066, 0.028, 0.0075]))
        .param(Param::color("knot_color", [0.024, 0.01, 0.003]))
        .param(Param::float("knots", 0.75).range(0.0, 1.0))
        .param(Param::float("tone", 0.4).range(0.0, 0.5))
        .node("unit", GraphInput::float("unit_id", 0.5))
        .node("lu", GraphInput::float("local_u", 0.5))
        .node("lv", GraphInput::float("local_v", 0.5))
        .node("len", GraphInput::float("length", 0.5))
        // The board frame in metres.
        .node("board_len", m(Mul, "len", p("repeat")))
        .node("x", m(Mul, "lu", "board_len"))
        .node("y", m(Mul, m(Sub, "lv", 0.5), p("board_width")))
        // The log: where its pith runs relative to this board.
        .node(
            "pith_y",
            m(Mul, m(Sub, hash("unit", 17.13, 0.31), 0.5), 0.4),
        )
        .node(
            "pith_z",
            m(Add, 0.045, m(Mul, hash("unit", 29.71, 0.57), 0.16)),
        )
        .node("taper", m(Mul, m(Sub, hash("unit", 41.37, 0.19), 0.5), 0.1))
        .node(
            "spacing",
            m(Add, 0.0026, m(Mul, hash("unit", 53.19, 0.77), 0.0028)),
        )
        // The rings wander at three scales: the log's sweep, a mid-scale
        // lean, and a wobble a few centimetres long.
        .node(
            "wander",
            Noise::perlin()
                .periods(4, 16)
                .octaves(3)
                .persistence(0.55)
                .seed(611),
        )
        .node("lean", Noise::perlin().periods(8, 32).octaves(2).seed(612))
        .node(
            "wiggle",
            Noise::perlin().periods(16, 64).octaves(2).seed(613),
        )
        .node(
            "across",
            m(
                Add,
                m(Sub, "y", "pith_y"),
                m(
                    Add,
                    m(Mul, m(Sub, "wander", 0.5), 0.014),
                    m(
                        Add,
                        m(Mul, m(Sub, "lean", 0.5), 0.0035),
                        m(Mul, m(Sub, "wiggle", 0.5), 0.0012),
                    ),
                ),
            ),
        )
        .node(
            "depth",
            m(Add, "pith_z", m(Mul, "taper", m(Sub, "x", 0.35))),
        )
        .node(
            "radius",
            Math::unary(
                Sqrt,
                m(Add, m(Mul, "across", "across"), m(Mul, "depth", "depth")),
            ),
        );
    let g = knot_slot(g, "a", 61.3, 0.41);
    let g = knot_slot(g, "b", 83.9, 0.67);
    let g = knot_slot(g, "c", 37.1, 0.29);
    let g = g
        // A large knot at `knots` odds, a small one at half, and a pin knot
        // at most boards.
        .node(
            "present_a",
            Math::new(Step, p("knots"), hash("unit", 71.9, 0.23)),
        )
        .node(
            "present_b",
            Math::new(Step, m(Mul, p("knots"), 0.6), hash("unit", 97.3, 0.83)),
        )
        .node(
            "present_c",
            Math::new(Step, m(Mul, p("knots"), 1.1), hash("unit", 23.9, 0.61)),
        )
        .node(
            "size_a",
            m(Add, 0.006, m(Mul, hash("unit", 13.7, 0.5), 0.009)),
        )
        .node(
            "size_b",
            m(Add, 0.0035, m(Mul, hash("unit", 19.3, 0.9), 0.0035)),
        )
        .node(
            "size_c",
            m(Add, 0.0012, m(Mul, hash("unit", 11.3, 0.4), 0.0012)),
        )
        .node("rel_a", m(Div, "knot_a", "size_a"))
        .node("rel_b", m(Div, "knot_b", "size_b"))
        .node("rel_c", m(Div, "knot_c", "size_c"))
        // The grain bends round a knot: a bump in the ring radius a few knot
        // radii wide and tall enough to close the rings on themselves.
        .node(
            "halo_a",
            m(
                Mul,
                "present_a",
                Math::unary(Exp2, m(Mul, m(Mul, "rel_a", "rel_a"), -0.22)),
            ),
        )
        .node(
            "halo_b",
            m(
                Mul,
                "present_b",
                Math::unary(Exp2, m(Mul, m(Mul, "rel_b", "rel_b"), -0.22)),
            ),
        )
        .node(
            "halo_c",
            m(
                Mul,
                "present_c",
                Math::unary(Exp2, m(Mul, m(Mul, "rel_c", "rel_c"), -0.3)),
            ),
        )
        .node(
            "swirl",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "halo_a", m(Mul, "size_a", 1.4)),
                    m(Mul, "halo_b", m(Mul, "size_b", 1.4)),
                ),
                m(Mul, "halo_c", m(Mul, "size_c", 1.2)),
            ),
        )
        // Rings pinch and spread: a noise in the phase itself moves a line
        // up to a ring either way, so neighbours crowd and part.
        .node("pinch", Noise::perlin().periods(8, 32).octaves(2).seed(617))
        .node(
            "phase",
            m(
                Add,
                m(Div, m(Add, "radius", "swirl"), "spacing"),
                m(
                    Add,
                    m(
                        Add,
                        m(Mul, hash("unit", 7.77, 0.13), 9.0),
                        m(Mul, m(Sub, "pinch", 0.5), 2.4),
                    ),
                    // Uneven growth: some years wide, some narrow.
                    m(
                        Mul,
                        Math::unary(Sin, m(Div, "radius", m(Mul, "spacing", 5.3))),
                        0.3,
                    ),
                ),
            ),
        )
        .node("ring", Math::unary(Fract, "phase"))
        // Early wood darkens gradually into the late band, which falls off
        // over an eighth of the ring: a soft edge rather than a ruled one.
        .node(
            "late_band",
            m(
                Mul,
                Math::new(Pow, ramp("ring", 0.5, 0.86), 1.6),
                Invert::new(ramp("ring", 0.88, 1.0)),
            ),
        )
        // The figure fades in and out along the board, and a line breaks into
        // streaks rather than running ruled from end to end.
        .node("fade", Noise::perlin().periods(4, 16).octaves(2).seed(621))
        .node(
            "breakup",
            Noise::perlin().periods(16, 128).octaves(2).seed(623),
        )
        .node(
            "contrast",
            m(
                Mul,
                remap(ramp("fade", 0.25, 0.75), 0.35, 1.0),
                remap(ramp("breakup", 0.25, 0.7), 0.45, 1.0),
            ),
        )
        .node("late", m(Mul, "late_band", "contrast"))
        .node(
            "core_a",
            m(Mul, "present_a", Invert::new(ramp("rel_a", 0.75, 1.0))),
        )
        .node(
            "core_b",
            m(Mul, "present_b", Invert::new(ramp("rel_b", 0.75, 1.0))),
        )
        .node(
            "core_c",
            m(Mul, "present_c", Invert::new(ramp("rel_c", 0.6, 1.0))),
        )
        .node(
            "rim_a",
            m(
                Mul,
                "present_a",
                m(
                    Mul,
                    ramp("rel_a", 0.55, 0.9),
                    Invert::new(ramp("rel_a", 1.0, 1.35)),
                ),
            ),
        )
        .node(
            "rim_b",
            m(
                Mul,
                "present_b",
                m(
                    Mul,
                    ramp("rel_b", 0.55, 0.9),
                    Invert::new(ramp("rel_b", 1.0, 1.35)),
                ),
            ),
        )
        // A check: a hairline split along the grain out of a large knot, on
        // about half of them.
        .node(
            "check",
            m(
                Mul,
                m(
                    Mul,
                    "present_a",
                    Math::new(Step, 0.5, hash("unit", 44.1, 0.37)),
                ),
                m(
                    Mul,
                    Invert::new(ramp(Math::unary(Abs, "ky_a"), 0.0002, 0.0007)),
                    m(
                        Mul,
                        Invert::new(Clamp::new(m(
                            Div,
                            m(Sub, Math::unary(Abs, "kx_a"), m(Mul, "size_a", 1.5)),
                            m(Mul, "size_a", 3.5),
                        ))),
                        ramp("rel_a", 0.9, 1.3),
                    ),
                ),
            ),
        )
        .node(
            "core",
            Clamp::new(m(Add, m(Add, "core_a", "core_b"), "core_c")),
        )
        .node("rim", Clamp::new(m(Add, "rim_a", "rim_b")))
        .node(
            "halo",
            Clamp::new(m(Add, m(Add, "halo_a", "halo_b"), "halo_c")),
        )
        // Fine streaks along the grain and open pores, no finer than 256
        // cells so a 512 bake still resolves them.
        .node(
            "fibre_noise",
            Noise::perlin().periods(32, 128).octaves(2).seed(619),
        )
        .node("fibre", ramp("fibre_noise", 0.2, 0.8))
        .node("tracheid", Noise::perlin().periods(64, 256).seed(627))
        .node("pore_noise", Noise::perlin().periods(64, 256).seed(631))
        .node("pores", ramp("pore_noise", 0.8, 0.92))
        .node(
            "mottle",
            Noise::perlin().periods(4, 32).octaves(3).seed(641),
        )
        // Per-board tone and hue: some boards much lighter or darker, a few
        // orange.
        .node(
            "board_tone",
            m(
                Add,
                1.0,
                m(
                    Mul,
                    m(Sub, hash("unit", 3.17, 0.71), 0.5),
                    m(Mul, p("tone"), 2.0),
                ),
            ),
        )
        .node("orange", hash("unit", 5.31, 0.37))
        .node(
            "board_hue",
            Mix::new(
                [1.0, 1.0, 1.0],
                [1.12, 1.0, 0.66],
                m(Mul, "orange", "orange"),
            ),
        )
        .node(
            "grain_color",
            Mix::new(p("early_color"), p("late_color"), "late"),
        )
        .node(
            "toned",
            m(
                Mul,
                m(Mul, "grain_color", m(Mul, "board_tone", "board_hue")),
                m(
                    Mul,
                    remap("mottle", 0.8, 1.15),
                    m(
                        Mul,
                        remap("fibre", 0.86, 1.08),
                        remap("tracheid", 0.93, 1.05),
                    ),
                ),
            ),
        )
        .node(
            "knotted",
            m(Mul, "toned", Invert::new(m(Mul, "halo", 0.18))),
        )
        .node(
            "rimmed",
            Mix::new("knotted", p("knot_color"), m(Mul, "rim", 0.85)),
        )
        .node(
            "cored",
            Mix::new("rimmed", m(Mul, p("late_color"), 0.7), m(Mul, "core", 0.92)),
        )
        .node(
            "checked",
            Mix::new("cored", p("knot_color"), m(Mul, "check", 0.9)),
        )
        .node(
            "color",
            Mix::new(
                "checked",
                m(Mul, p("late_color"), 0.5),
                m(Mul, "pores", 0.35),
            ),
        )
        .node(
            "height",
            m(
                Sub,
                m(
                    Add,
                    m(Add, 0.5, m(Mul, "late", 0.012)),
                    m(
                        Add,
                        m(Mul, m(Add, "fibre", "tracheid"), 0.003),
                        m(Mul, "core", 0.006),
                    ),
                ),
                m(
                    Add,
                    m(Add, m(Mul, "pores", 0.006), m(Mul, "rim", 0.004)),
                    m(Mul, "check", 0.03),
                ),
            ),
        )
        .node(
            "roughness",
            m(
                Add,
                m(Sub, 0.66, m(Mul, "late", 0.06)),
                m(
                    Add,
                    m(Sub, m(Mul, "pores", 0.12), m(Mul, "core", 0.08)),
                    m(Mul, "check", 0.2),
                ),
            ),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0)
            .extra("knots", "core")
            .extra("knot_rims", "rim")
            .extra("late", "late")
            .extra("fibre", "fibre"),
        1.3,
        0.02,
    )
}

/// Varnished pine boards in random lengths with narrow dark joints.
///
/// Boards darken toward their edges where grime has worked in, are face
/// nailed near both ends, and carry a fine dent and scratch texture in the
/// roughness under broad worn traffic lanes.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn wood_floor() -> MaterialGraph {
    use MathOp::{Abs, Add, Min, Mul, Sqrt, Step, Sub};
    let g = MaterialGraph::builder("library:wood-floor")
        .param(Param::float("varnish", 0.8).range(0.0, 1.0))
        .param(Param::float("wear", 0.5).range(0.0, 1.0))
        .layer("layout", Subgraph::new("layouts:board-lattice"), &fields())
        .layer("timber", timber_on_boards("layout"), &grain())
        // A board is not quite flat: a little cupping across it and a
        // per-board setting, each a fraction of a millimetre.
        .node(
            "cup",
            m(
                Mul,
                m(
                    Mul,
                    m(Sub, "layout.local_v", 0.5),
                    m(Sub, "layout.local_v", 0.5),
                ),
                -0.04,
            ),
        )
        // Face nails, two a board end, 22 mm in from the end and 45 mm either
        // side of the centre line, on four boards in five.
        .node("board_m", m(Mul, "layout.length", 1.3))
        .node("along_m", m(Mul, "layout.local_u", "board_m"))
        .node("from_end", m(Min, "along_m", m(Sub, "board_m", "along_m")))
        .node(
            "nail_y",
            m(
                Sub,
                Math::unary(Abs, m(Mul, m(Sub, "layout.local_v", 0.5), 0.1625)),
                0.045,
            ),
        )
        .node("nail_x", m(Sub, "from_end", 0.022))
        .node(
            "nail_d",
            Math::unary(
                Sqrt,
                m(Add, m(Mul, "nail_x", "nail_x"), m(Mul, "nail_y", "nail_y")),
            ),
        )
        .node(
            "nailed",
            Math::new(Step, 0.8, hash("layout.unit_id", 9.13, 0.47)),
        )
        .node(
            "nail",
            m(
                Mul,
                Invert::new("nailed"),
                Invert::new(ramp("nail_d", 0.0011, 0.0017)),
            ),
        )
        .node(
            "nail_stain",
            m(
                Mul,
                Invert::new("nailed"),
                Invert::new(ramp("nail_d", 0.0015, 0.0045)),
            ),
        )
        // Small dents from dropped things, which dull the varnish and dip
        // the face a fraction of a millimetre.
        .node(
            "dent_noise",
            Noise::perlin().periods(64, 64).octaves(2).seed(667),
        )
        .node("dents", m(Mul, p("wear"), ramp("dent_noise", 0.66, 0.74)))
        .node(
            "face",
            m(
                Sub,
                m(
                    Add,
                    m(Add, 0.62, "cup"),
                    m(
                        Add,
                        m(Sub, "timber.height", 0.5),
                        remap("layout.unit_id", -0.012, 0.012),
                    ),
                ),
                m(Add, m(Mul, "nail", 0.03), m(Mul, "dents", 0.006)),
            ),
        )
        .node("dust", Noise::value().periods(64, 256).octaves(2).seed(653))
        .node("joint_height", m(Add, 0.28, m(Mul, "dust", 0.08)))
        .node("height", Mix::new("joint_height", "face", "layout.mask"))
        // Foot traffic: broad worn lanes where the varnish has dulled, and
        // scuffs and fine scratches across them.
        .node(
            "traffic",
            Noise::perlin().periods(2, 4).octaves(3).seed(659),
        )
        .node(
            "worn",
            Clamp::new(m(Mul, p("wear"), ramp("traffic", 0.35, 0.8))),
        )
        .node(
            "scuffs",
            Scratches::new()
                .count(96)
                .length(0.08)
                .width(0.0007)
                .angle(0.0)
                .angle_spread(40.0)
                .seed(661),
        )
        .node(
            "fine_scratches",
            Scratches::new()
                .count(256)
                .length(0.025)
                .width(0.0004)
                .angle(0.0)
                .angle_spread(180.0)
                .seed(663),
        )
        .node("scuffed", m(Mul, "scuffs", m(Mul, p("wear"), 0.8)))
        .node(
            "joint_color",
            Mix::new(
                [0.012, 0.007, 0.003],
                [0.045, 0.04, 0.034],
                ramp("dust", 0.6, 0.92),
            ),
        )
        // Grime worked in toward every board edge: darker brown within the
        // last centimetre and a half.
        .node(
            "edge_grime",
            remap(
                ramp(m(Mul, "layout.edge_distance", 1.3), 0.0, 0.016),
                0.6,
                1.0,
            ),
        )
        .node(
            "face_color",
            m(
                Mul,
                "timber.base_color",
                m(
                    Mul,
                    "edge_grime",
                    m(
                        Add,
                        1.0,
                        m(Add, m(Mul, "worn", 0.1), m(Mul, "scuffed", 0.25)),
                    ),
                ),
            ),
        )
        .node(
            "stained",
            m(Mul, "face_color", Invert::new(m(Mul, "nail_stain", 0.45))),
        )
        .node(
            "nailed_color",
            Mix::new("stained", [0.022, 0.02, 0.018], "nail"),
        )
        .node(
            "edged",
            m(Mul, "nailed_color", remap("layout.mask", 0.55, 1.0)),
        )
        .node("color", Mix::new("joint_color", "edged", "layout.mask"))
        .node(
            "varnished",
            Mix::new("timber.roughness", 0.24, p("varnish")),
        )
        .node(
            "dulled",
            m(
                Add,
                "varnished",
                m(
                    Add,
                    m(Add, m(Mul, "worn", 0.16), m(Mul, "scuffed", 0.3)),
                    m(
                        Add,
                        m(Mul, "dents", 0.1),
                        m(Mul, "fine_scratches", m(Mul, p("wear"), 0.24)),
                    ),
                ),
            ),
        )
        .node(
            "roughness",
            Mix::new(0.85, Mix::new("dulled", 0.45, "nail"), "layout.mask"),
        );
    finish(
        g,
        PbrOutput::new()
            .base_color("color")
            .roughness("roughness")
            .metallic(0.0),
        1.3,
        0.02,
    )
}

/// Painted lap siding, sharing the paint film with metal. Each course is a
/// wedge, thick at its lower edge where it laps the course below; the paint
/// weathers off along the grain and the drip edge first, exposing silvered
/// timber, and a peel lifts the film where it has let go.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn painted_boards() -> MaterialGraph {
    use MathOp::{Add, Mul, Pow, Sub};
    let g = MaterialGraph::builder("library:painted-boards")
        .param(Param::float("age", 0.45).range(0.0, 1.0))
        .param(Param::color("color", [0.6, 0.52, 0.29]))
        .layer(
            "layout",
            Subgraph::new("layouts:board-lattice").param("split", ParamValue::Float(0.0)),
            &fields(),
        )
        .layer(
            "timber",
            timber_on_boards("layout")
                .param("knots", ParamValue::Float(0.55))
                .param("tone", ParamValue::Float(0.1))
                .param("early_color", ParamValue::Color([0.71, 0.67, 0.48]))
                .param("late_color", ParamValue::Color([0.56, 0.51, 0.34]))
                .param("knot_color", ParamValue::Color([0.24, 0.17, 0.07])),
            &grain(),
        )
        // The lap: thin under the course above, thick at the butt, a 17 mm
        // step with only a small roundover on the drip edge.
        .node(
            "wedge",
            m(
                Sub,
                m(Add, 0.04, m(Mul, "layout.local_v", 0.92)),
                m(
                    Mul,
                    Math::new(Pow, ramp("layout.local_v", 0.97, 1.0), 2.0),
                    0.07,
                ),
            ),
        )
        // The shadow the butt above casts on the top of each course, and the
        // grime that collects there because the rain does not reach it.
        .node(
            "under_lap",
            Math::new(Pow, Invert::new(ramp("layout.local_v", 0.0, 0.075)), 1.3),
        )
        // Rough-sawn face: fine cross-fibre tear.
        .node("sawn", Noise::perlin().periods(128, 256).seed(673))
        .node(
            "board_face",
            m(
                Add,
                "wedge",
                m(
                    Add,
                    m(Mul, m(Sub, "timber.height", 0.5), 0.7),
                    m(Mul, m(Sub, "sawn", 0.5), 0.008),
                ),
            ),
        )
        .node(
            "board_height",
            Mix::new(0.06, "board_face", "layout.end_mask"),
        )
        // Paint lets go along the early wood and the fibre first, in broad
        // weathered patches, and at the drip edge.
        .node(
            "weather",
            Noise::perlin().periods(4, 16).octaves(3).seed(677),
        )
        .node(
            "failure",
            m(
                Add,
                m(
                    Add,
                    m(Mul, Invert::new("timber.late"), 0.45),
                    m(Mul, "timber.fibre", 0.14),
                ),
                m(
                    Sub,
                    m(Mul, "weather", 0.4),
                    m(Mul, ramp("layout.local_v", 0.8, 1.0), 0.14),
                ),
            ),
        )
        .node(
            "loss",
            Clamp::new(m(Mul, m(Sub, m(Mul, p("age"), 1.3), "failure"), 25.0)),
        )
        .node(
            "wood_color",
            m(
                Mul,
                "timber.base_color",
                m(
                    Mul,
                    m(
                        Mul,
                        remap("layout.end_mask", 0.3, 1.0),
                        Invert::new(m(Mul, "under_lap", 0.55)),
                    ),
                    remap("sawn", 0.94, 1.05),
                ),
            ),
        )
        // Paint picks up the grain beneath it and a resin bleed over knots.
        // Every field the paint reads that does not move with `color` or
        // `age` is folded into one shade and one weight here, so a compiled
        // variant with both live samples two planes for them rather than
        // one for each ingredient.
        .node(
            "bleed",
            Clamp::new(m(Add, "timber.knot_rims", m(Mul, "timber.knots", 0.4))),
        )
        .node(
            "shade",
            m(
                Mul,
                m(
                    Mul,
                    m(
                        Mul,
                        remap("timber.late", 1.02, 0.95),
                        remap("weather", 0.92, 1.05),
                    ),
                    m(
                        Mul,
                        remap("layout.end_mask", 0.45, 1.0),
                        remap("sawn", 0.94, 1.05),
                    ),
                ),
                // Grime under the lap, where the rain does not wash.
                Invert::new(m(Mul, "under_lap", 0.72)),
            ),
        )
        .node(
            "paint_color",
            m(
                Mul,
                Mix::new(p("color"), [0.4, 0.28, 0.09], "bleed"),
                "shade",
            ),
        )
        // Weathered film: in broad patches along the grain the coat has
        // chalked thin and the silvered early wood shows through it.
        .node("chalk", Noise::perlin().periods(8, 32).octaves(4).seed(683))
        .node(
            "chalky",
            m(
                Mul,
                ramp("chalk", 0.35, 0.8),
                m(Add, 0.25, m(Mul, Invert::new("timber.late"), 0.5)),
            ),
        )
        .node("thin", Clamp::new(m(Mul, m(Mul, p("age"), 1.4), "chalky")))
        .node(
            "thinned",
            Mix::new("paint_color", "wood_color", m(Mul, "thin", 0.8)),
        )
        .layer(
            "film",
            Subgraph::new("weathering:paint_film")
                .input("base_color", "wood_color")
                .input("roughness", 0.82)
                .input("metallic", 0.0)
                .input("height", "board_height")
                .input("paint_color", "thinned")
                .input("loss_mask", "loss")
                .param("paint_roughness", ParamValue::Float(0.68)),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
            ],
        )
        .layer(
            "peel",
            Subgraph::new("weathering:peeling_paint")
                .input("height", "film.height")
                .input("paint_color", "film.base_color")
                .input("paint_roughness", "film.roughness")
                .input("substrate_color", "wood_color")
                .input("substrate_roughness", 0.82)
                .input("peel_mask", "loss")
                .param("lip", ParamValue::Float(0.006))
                .param("curl_width", ParamValue::Float(0.0012)),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Height,
            ],
        )
        .node("height", Clamp::new("peel.height"))
        // The grain reads through the film mostly as relief and sheen: the
        // coat is a touch glossier over late wood and rougher over torn fibre.
        .node(
            "roughness",
            Clamp::new(m(
                Add,
                "peel.roughness",
                m(
                    Add,
                    m(Mul, m(Sub, 0.5, "timber.late"), 0.1),
                    m(Mul, m(Sub, "sawn", 0.5), 0.08),
                ),
            )),
        )
        .node("physical_height", m(Mul, "height", 0.02 / 1.3))
        .node(
            "relief_ao",
            OcclusionFromHeight::new("physical_height").radius(0.016),
        )
        .node(
            "ao",
            m(Mul, "relief_ao", Invert::new(m(Mul, "under_lap", 0.75))),
        );
    g.tile_metres([1.3; 2])
        .output(
            PbrOutput::new()
                .base_color("peel.base_color")
                .roughness("roughness")
                .metallic(0.0)
                .height("height")
                .occlusion("ao")
                .normal_strength(0.02 / 1.3)
                .extra("loss", "loss"),
        )
        .into_graph()
}
