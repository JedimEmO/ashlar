//! The city surfaces: glazed curtain wall, banded cladding, a marked road,
//! sci-fi interior panelling and a glowing advertising panel.
//!
//! The two facades are laid out for the city kit's grid, 4.0 m bays and 3.8 m
//! storeys, under box UVs from the building origin, so a tower's windows land
//! on its storeys. Each repeat is four bays by four storeys; that is what
//! keeps a lit office from repeating visibly up a tower. The layouts are UV
//! arithmetic rather than generators, so the storey and bay lines are exact.
//!
//! A live parameter reaches only the last multiply or threshold of what it
//! moves: `tint` and `color` scale a baked plane, and `lit` thresholds a baked
//! rank, so a compiled twin with every parameter live binds a few images.
use super::surfaces::{CHANNELS, m, p, remap};
use crate::{
    Channel, Input, MaterialGraph, MaterialGraphBuilder, MathOp, Param, ParamValue, PbrOutput,
    SurfaceOutput,
    nodes::{
        Clamp, Decompose, Invert, Levels, Math, Mix, Noise, OcclusionFromHeight, Scratches,
        Subgraph, Uv,
    },
};

/// `clamp((a - b) / width)`: zero at or below `b`, one from `b + width` up.
fn ramp(a: impl Into<Input>, b: impl Into<Input>, width: f32) -> Clamp {
    Clamp::new(m(MathOp::Div, m(MathOp::Sub, a, b), width))
}

/// One inside `lo..hi` of `x`, falling to zero over `soft` either side.
fn band(x: &str, lo: f32, hi: f32, soft: f32) -> Math {
    m(MathOp::Mul, ramp(x, lo, soft), ramp(hi, x, soft))
}

/// A stable hash in `0..1` of two cell indices.
fn hash(a: impl Into<Input>, b: impl Into<Input>, salt: f32) -> Math {
    use MathOp::{Add, Fract, Mul, Sin};
    Math::unary(
        Fract,
        m(
            Mul,
            Math::unary(
                Sin,
                m(Add, m(Add, m(Mul, a, 0.1731), m(Mul, b, 0.7549)), salt),
            ),
            4375.31,
        ),
    )
}

/// Height, occlusion and normal strength for a repeat of `tile` metres whose
/// height field spans `depth` metres. The normal strength is relief per UV
/// unit along the wider axis; the two axes of the facades differ by 5 %.
fn finish_rect(
    g: MaterialGraphBuilder,
    output: PbrOutput,
    tile: [f32; 2],
    depth: f32,
) -> MaterialGraph {
    let wide = tile[0].max(tile[1]);
    g.node("physical_height", m(MathOp::Mul, "height", depth / wide))
        .node(
            "ao",
            OcclusionFromHeight::new("physical_height").radius(0.012),
        )
        .tile_metres(tile)
        .output(
            output
                .height("height")
                .occlusion("ao")
                .normal_strength(depth / wide),
        )
        .into_graph()
}

/// The UV components, and the cell coordinates of a facade laid out as four
/// 4 m bays of four 1 m panes across, by four 3.8 m storeys up.
///
/// Leaves `u`, `v`, `col` (pane column 0..16), `lx` (0..1 across the pane),
/// `bay` (0..4), `bay_x` (metres into the bay), `storey` (0..4) and `y`
/// (metres up the storey).
fn facade_grid(g: MaterialGraphBuilder) -> MaterialGraphBuilder {
    use MathOp::{Floor, Fract, Mul};
    g.node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("col", Math::unary(Floor, m(Mul, "u", 16.0)))
        .node("lx", Math::unary(Fract, m(Mul, "u", 16.0)))
        .node("bay", Math::unary(Floor, m(Mul, "u", 4.0)))
        .node("bay_x", m(Mul, Math::unary(Fract, m(Mul, "u", 4.0)), 4.0))
        .node("storey", Math::unary(Floor, m(Mul, "v", 4.0)))
        .node("y", m(Mul, Math::unary(Fract, m(Mul, "v", 4.0)), 3.8))
}

/// Vision glass seen from outside, at `sill..head` metres up the storey:
/// the dark reflective pane, blinds dropped to a random depth behind a share
/// of them, and the interior light of the lit ones.
///
/// Leaves `glass_color` (unlit albedo, before the tint), `glass_roughness`,
/// `rank` (a pane is lit where `lit` exceeds it; offices light together) and
/// `light` (the emitted colour where lit, blinds and fittings included).
#[expect(
    clippy::too_many_lines,
    reason = "one graph fragment, in field dependency order"
)]
fn vision(
    g: MaterialGraphBuilder,
    sill: f32,
    head: f32,
    seed: u32,
    seed_f: f32,
) -> MaterialGraphBuilder {
    use MathOp::{Add, Div, Fract, Max, Mul, Pow, Step, Sub};
    g.node("pane_id", hash("col", "storey", 0.391 + seed_f))
        .node("office_id", hash("bay", "storey", 0.713 + seed_f))
        // Height up the pane, zero at the sill and one at the head.
        .node("vy", Clamp::new(m(Div, m(Sub, "y", sill), head - sill)))
        // The reflection: panes are never quite coplanar, so each throws back
        // a slightly different piece of sky, lighter towards its top.
        .node(
            "sky",
            remap(
                Noise::perlin().period(2).octaves(3).seed(seed + 1),
                0.8,
                1.2,
            ),
        )
        .node(
            "reflect",
            m(
                Mul,
                m(Mul, "sky", remap("pane_id", 0.78, 1.22)),
                m(Add, 0.88, m(Mul, "vy", 0.22)),
            ),
        )
        // Blinds: a share of panes has them, dropped a random depth from
        // the head, their slats a faint light band behind the glass.
        .node(
            "has_blind",
            m(Step, hash("col", "storey", 1.37 + seed_f), 0.55),
        )
        .node(
            "blind_drop",
            remap(hash("col", "storey", 2.11 + seed_f), 0.15, 0.95),
        )
        .node(
            "blind",
            m(
                Mul,
                "has_blind",
                ramp("vy", m(Sub, 1.0, "blind_drop"), 0.01),
            ),
        )
        .node(
            "slats",
            remap(Math::unary(Fract, m(Mul, "v", 4.0 * 3.8 / 0.05)), 0.72, 1.0),
        )
        .node(
            "glass_color",
            Mix::new(
                m(Mul, [0.024, 0.042, 0.052], "reflect"),
                m(Mul, [0.12, 0.115, 0.1], "slats"),
                m(Mul, "blind", 0.4),
            ),
        )
        // Water that ran down the glass from the transom above.
        .node(
            "runs",
            m(
                Mul,
                Levels::new(Noise::perlin().periods(256, 8).octaves(2).seed(seed + 3))
                    .in_range(0.55, 0.8),
                m(Pow, "vy", 3.0),
            ),
        )
        .node(
            "glass_roughness",
            m(
                Add,
                m(Add, 0.05, m(Mul, "pane_id", 0.04)),
                m(Mul, "runs", 0.16),
            ),
        )
        // Lighting: offices light together, with a few panes of their own.
        // A blend of two uniforms bunches towards the middle; spreading it
        // back out keeps the lit share close to what `lit` asks for.
        .node(
            "rank",
            Levels::new(Mix::new(
                "office_id",
                hash("col", "storey", 3.07 + seed_f),
                0.22,
            ))
            .in_range(0.11, 0.89),
        )
        .node(
            "warmth",
            Mix::new(
                Mix::new(
                    [1.0, 0.7, 0.4],
                    [0.95, 0.9, 0.8],
                    m(Step, "office_id", 0.55),
                ),
                [0.66, 0.8, 1.0],
                m(Step, hash("bay", "storey", 4.3 + seed_f), 0.82),
            ),
        )
        // Ceiling luminaires near the head, a glow falling towards the
        // sill, and furniture a dark band low down.
        .node(
            "fittings",
            m(
                Max,
                m(Mul, band("vy", 0.9, 0.95, 0.01), 1.4),
                m(Add, 0.28, m(Mul, m(Pow, "vy", 1.6), 0.7)),
            ),
        )
        .node(
            "furniture",
            m(Sub, 1.0, m(Mul, band("vy", 0.0, 0.3, 0.02), 0.45)),
        )
        .node(
            "light",
            m(
                Mul,
                "warmth",
                m(
                    Mul,
                    m(Mul, "fittings", "furniture"),
                    m(
                        Mul,
                        remap("pane_id", 0.6, 1.0),
                        Mix::new(1.0, m(Mul, "slats", 0.55), "blind"),
                    ),
                ),
            ),
        )
}

/// A glazed curtain wall: aluminium mullions and transoms on a 1 m pane grid
/// in 4 m bays, an opaque spandrel over each slab, a top light under each
/// head, and vision glass with blinds and lit offices behind it.
///
/// Four bays by four storeys, 16 by 15.2 m, laid out for the city kit's grid
/// under box UVs. The frame stands 5 cm proud of the glass over an 8 cm
/// height range. `tint` scales the glass; `lit` is the fraction of panes lit.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn curtain_wall() -> MaterialGraph {
    use MathOp::{Abs, Add, Max, Min, Mul, Pow, Sqrt, Step, Sub};
    const SPANDREL: f32 = 0.9;
    const TOP_LIGHT: f32 = 3.25;
    let g = MaterialGraph::builder("library:curtain-wall")
        .param(Param::color("tint", [1.0, 1.0, 1.0]))
        .param(Param::float("lit", 0.32).range(0.0, 1.0));
    let g = facade_grid(g);
    let g = vision(g, SPANDREL, TOP_LIGHT, 1301, 0.17)
        // Metres to the nearest mullion, bay mullion and transom.
        .node("pane_edge", m(Min, "lx", Invert::new("lx")))
        .node("bay_edge", m(Min, "bay_x", m(Sub, 4.0, "bay_x")))
        .node("slab_edge", m(Min, "y", m(Sub, 3.8, "y")))
        .node(
            "transom_edge",
            m(
                Min,
                Math::unary(Abs, m(Sub, "y", SPANDREL)),
                Math::unary(Abs, m(Sub, "y", TOP_LIGHT)),
            ),
        )
        // Each member a rounded cap, highest on its centre line.
        .node(
            "mullion",
            Math::unary(
                Sqrt,
                Clamp::new(m(Sub, 1.0, m(Mul, "pane_edge", 1.0 / 0.03))),
            ),
        )
        .node(
            "bay_mullion",
            Math::unary(
                Sqrt,
                Clamp::new(m(Sub, 1.0, m(Mul, "bay_edge", 1.0 / 0.055))),
            ),
        )
        .node(
            "slab_transom",
            Math::unary(
                Sqrt,
                Clamp::new(m(Sub, 1.0, m(Mul, "slab_edge", 1.0 / 0.055))),
            ),
        )
        .node(
            "transom",
            Math::unary(
                Sqrt,
                Clamp::new(m(Sub, 1.0, m(Mul, "transom_edge", 1.0 / 0.03))),
            ),
        )
        .node(
            "frame",
            m(
                Max,
                m(Max, m(Mul, "mullion", 0.85), "bay_mullion"),
                m(Max, "slab_transom", m(Mul, "transom", 0.85)),
            ),
        )
        .node("frame_mask", ramp("frame", 0.0, 0.08))
        // Zones: the spandrel over the slab, the top light under the head.
        .node("spandrel", m(Step, SPANDREL, "y"))
        .node("top_light", m(Step, "y", TOP_LIGHT))
        .node(
            "glazed",
            m(Mul, Invert::new("frame_mask"), Invert::new("spandrel")),
        )
        .node(
            "spandrel_face",
            m(Mul, Invert::new("frame_mask"), "spandrel"),
        )
        // The frame: anodised aluminium, dulled where dirt settles on the
        // top of a transom.
        .node(
            "frame_dirt",
            m(
                Mul,
                Levels::new(Noise::perlin().period(64).octaves(3).seed(1307)).in_range(0.35, 0.8),
                m(Max, "slab_transom", "transom"),
            ),
        )
        .node(
            "frame_color",
            m(
                Mul,
                [0.5, 0.515, 0.53],
                m(
                    Mul,
                    remap(Noise::perlin().period(16).octaves(2).seed(1309), 0.9, 1.05),
                    m(Sub, 1.0, m(Mul, "frame_dirt", 0.35)),
                ),
            ),
        )
        // The spandrel: fritted opaque glass, a shade lighter than the vision
        // glass and a little rougher, varying bay to bay.
        .node(
            "spandrel_color",
            m(
                Mul,
                [0.052, 0.058, 0.064],
                m(Mul, remap("office_id", 0.85, 1.15), remap("sky", 0.9, 1.1)),
            ),
        )
        // The top light shares its pane's lighting but never its blind.
        .node(
            "top_glass",
            m(
                Mul,
                [0.028, 0.048, 0.06],
                m(Mul, "sky", remap("pane_id", 0.8, 1.2)),
            ),
        )
        .node(
            "vision_color",
            Mix::new("glass_color", "top_glass", "top_light"),
        )
        // Albedo as two planes: glass the tint scales, and the rest.
        .node("glass_plane", m(Mul, "vision_color", "glazed"))
        .node(
            "rest_plane",
            m(
                Add,
                m(Mul, "frame_color", "frame_mask"),
                m(Mul, "spandrel_color", "spandrel_face"),
            ),
        )
        .node(
            "color_out",
            m(Add, m(Mul, p("tint"), "glass_plane"), "rest_plane"),
        )
        .node(
            "roughness",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "glass_roughness", "glazed"),
                    m(Mul, 0.24, "spandrel_face"),
                ),
                m(Mul, m(Add, 0.3, m(Mul, "frame_dirt", 0.25)), "frame_mask"),
            ),
        )
        .node(
            "metallic",
            m(
                Add,
                m(Mul, 0.5, "glazed"),
                m(
                    Add,
                    m(Mul, 0.3, "spandrel_face"),
                    m(Mul, 0.85, "frame_mask"),
                ),
            ),
        )
        // The light, where the pane is glazed, dimmed a little in the top
        // light, cut by `lit`.
        .node(
            "light_plane",
            m(
                Mul,
                "light",
                m(Mul, "glazed", Mix::new(1.0, 0.7, "top_light")),
            ),
        )
        .node("lit_mask", ramp(p("lit"), "rank", 0.004))
        .node("emissive", m(Mul, "light_plane", "lit_mask"))
        .node(
            "height",
            m(
                Add,
                m(Add, 0.36, m(Mul, "spandrel_face", 0.015)),
                m(Mul, m(Pow, "frame", 0.7), 0.6),
            ),
        );
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic("metallic")
            .emissive("emissive"),
        [16.0, 15.2],
        0.08,
    )
}

/// Banded cladding: precast panels on each storey, a ribbon window set back
/// behind a projecting sill, and grime run down from the sill ends.
///
/// Four bays by four storeys, 16 by 15.2 m, on the city kit's grid. The
/// window is recessed 12 cm behind the panel face over a 16 cm height range.
/// `color` scales the cladding; `lit` is the fraction of panes lit.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn window_band() -> MaterialGraph {
    use MathOp::{Abs, Add, Max, Min, Mul, Pow, Sqrt, Step, Sub};
    const SILL: f32 = 1.12;
    const HEAD: f32 = 3.05;
    let g = MaterialGraph::builder("library:window-band")
        .param(Param::color("color", [0.4, 0.388, 0.36]))
        .param(Param::float("lit", 0.3).range(0.0, 1.0));
    let g = facade_grid(g);
    let g = vision(g, SILL, HEAD, 1401, 0.29)
        .node("window", band("y", SILL, HEAD, 0.004))
        // Window frames: a thin mullion every metre and a frame at sill and
        // head, dark grey powder coat.
        .node("pane_edge", m(Min, "lx", Invert::new("lx")))
        .node(
            "frame_edge",
            m(
                Min,
                "pane_edge",
                m(
                    Min,
                    Math::unary(Abs, m(Sub, "y", SILL)),
                    Math::unary(Abs, m(Sub, "y", HEAD)),
                ),
            ),
        )
        .node(
            "frame",
            m(
                Mul,
                "window",
                Math::unary(
                    Sqrt,
                    Clamp::new(m(Sub, 1.0, m(Mul, "frame_edge", 1.0 / 0.028))),
                ),
            ),
        )
        .node("frame_mask", ramp("frame", 0.0, 0.1))
        .node("glazed", m(Mul, "window", Invert::new("frame_mask")))
        // The sill: a 7 cm projecting nose under the window, lit on top.
        .node("sill", band("y", SILL - 0.08, SILL, 0.006))
        .node("sill_top", band("y", SILL - 0.025, SILL, 0.004))
        // Panel joints: a sealant line at each storey line and each bay line,
        // in the cladding only.
        .node(
            "joint_edge",
            m(
                Min,
                m(Min, "bay_x", m(Sub, 4.0, "bay_x")),
                m(Min, "y", m(Sub, 3.8, "y")),
            ),
        )
        .node("joint", Invert::new(ramp("joint_edge", 0.008, 0.004)))
        .node(
            "cladding",
            m(
                Mul,
                Invert::new("window"),
                m(Mul, Invert::new("joint"), Invert::new("sill")),
            ),
        )
        // The precast face: per-panel tone, a slow blotch, fine grain.
        .node("panel_id", hash("bay", "storey", 5.17))
        .node(
            "blotch",
            remap(Noise::perlin().period(8).octaves(4).seed(1403), 0.88, 1.08),
        )
        .node(
            "grain",
            remap(Noise::value().period(512).octaves(1).seed(1405), 0.94, 1.04),
        )
        // Grime: water off a sill carries dirt down the panel below it, most
        // under a sill end and a mullion, in streaks.
        .node(
            "below_sill",
            m(
                Pow,
                Clamp::new(m(Sub, 1.0, Math::unary(Abs, m(Sub, "y", SILL - 0.08)))),
                2.0,
            ),
        )
        .node(
            "below_sill_masked",
            m(Mul, "below_sill", m(Step, SILL, "y")),
        )
        .node(
            "under_mullion",
            m(
                Add,
                0.35,
                m(Mul, Clamp::new(m(Sub, 1.0, m(Mul, "pane_edge", 3.0))), 0.65),
            ),
        )
        .node(
            "streaks",
            Levels::new(Noise::perlin().periods(128, 4).octaves(3).seed(1407)).in_range(0.42, 0.78),
        )
        .node(
            "grime",
            Clamp::new(m(
                Add,
                m(
                    Mul,
                    m(Mul, "below_sill_masked", "under_mullion"),
                    m(Mul, "streaks", 0.9),
                ),
                m(
                    Mul,
                    Levels::new(Noise::perlin().period(4).octaves(3).seed(1409)).in_range(0.5, 0.9),
                    0.25,
                ),
            )),
        )
        .node(
            "clad_tone",
            m(
                Mul,
                m(Mul, m(Mul, remap("panel_id", 0.9, 1.08), "blotch"), "grain"),
                m(Sub, 1.0, m(Mul, "grime", 0.45)),
            ),
        )
        .node("clad_plane", m(Mul, "clad_tone", "cladding"))
        .node(
            "rest_plane",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "glass_color", "glazed"),
                    m(Mul, [0.07, 0.072, 0.075], "frame_mask"),
                ),
                m(
                    Add,
                    m(
                        Mul,
                        m(
                            Mul,
                            [0.4, 0.39, 0.37],
                            m(Add, 0.8, m(Mul, "sill_top", 0.35)),
                        ),
                        "sill",
                    ),
                    m(Mul, [0.03, 0.03, 0.03], "joint"),
                ),
            ),
        )
        .node(
            "color_out",
            m(Add, m(Mul, p("color"), "clad_plane"), "rest_plane"),
        )
        .node(
            "roughness",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "glass_roughness", "glazed"),
                    m(Mul, 0.42, "frame_mask"),
                ),
                m(
                    Add,
                    m(Mul, m(Add, 0.82, m(Mul, "grime", 0.1)), "cladding"),
                    m(Add, m(Mul, 0.6, "sill"), m(Mul, 0.9, "joint")),
                ),
            ),
        )
        .node(
            "metallic",
            m(Add, m(Mul, 0.5, "glazed"), m(Mul, 0.4, "frame_mask")),
        )
        .node("light_plane", m(Mul, "light", "glazed"))
        .node("lit_mask", ramp(p("lit"), "rank", 0.004))
        .node("emissive", m(Mul, "light_plane", "lit_mask"))
        // Panel face 0.75, sill nose proud to 0.95, the glass set back to
        // 0.0 and the frame 3 cm in front of it, the joints 1.5 cm in.
        .node(
            "height",
            m(
                Max,
                m(
                    Add,
                    m(
                        Mul,
                        m(Add, 0.75, remap("blotch", -0.01, 0.01)),
                        m(Mul, Invert::new("window"), Invert::new("joint")),
                    ),
                    m(Mul, 0.66, "joint"),
                ),
                m(
                    Max,
                    m(Mul, "sill", m(Add, 0.9, m(Mul, "sill_top", 0.05))),
                    m(Mul, "window", m(Add, 0.02, m(Mul, "frame", 0.2))),
                ),
            ),
        );
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic("metallic")
            .emissive("emissive"),
        [16.0, 15.2],
        0.16,
    )
}

/// A marked carriageway, 12 m across under box UVs (U across, V along).
///
/// The substrate is `library:asphalt`, instanced whole: its aggregate, its
/// crack network and its smears land at road scale across the 12 m repeat.
/// Over it: white edge lines 0.6 m in from each side and a dashed centre
/// line (3 m dash, 3 m gap), worn thin where tyres run; the wheel paths of
/// both lanes darkened and polished; oil dropped between them; a sealed
/// construction joint under the centre line and two sealed transverse
/// cracks; and a fresher rectangular repair in one lane. `wear` wears the
/// paint. `wet` is rain: the whole carriageway darkened and smoothed, and
/// water standing in the ruts, along the gutters and in the sags; at zero it
/// is the dry bake exactly.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn road() -> MaterialGraph {
    use MathOp::{Abs, Add, Fract, Max, Min, Mul, Pow, Step, Sub};
    let asphalt = |output: SurfaceOutput| Subgraph::new("library:asphalt").output(output);
    let g = MaterialGraph::builder("library:road")
        .param(Param::float("wear", 0.45).range(0.0, 1.0))
        .param(Param::float("wet", 0.0).range(0.0, 1.0))
        .node("asphalt_color", asphalt(SurfaceOutput::BaseColor))
        .node("asphalt_roughness", asphalt(SurfaceOutput::Roughness))
        .node("asphalt_height", asphalt(SurfaceOutput::Height))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("x", m(Mul, "u", 12.0))
        .node("y", m(Mul, "v", 12.0))
        // A slow wander, so no line is ruled.
        .node(
            "wobble",
            remap(
                Noise::perlin().periods(1, 8).octaves(2).seed(1501),
                -0.03,
                0.03,
            ),
        )
        .node("xw", m(Add, "x", "wobble"))
        // Wheel paths: two per lane, 0.9 m either side of its centre.
        .node(
            "track",
            m(
                Max,
                m(
                    Max,
                    m(
                        Pow,
                        Clamp::new(m(
                            Sub,
                            1.0,
                            m(Mul, Math::unary(Abs, m(Sub, "xw", 2.4)), 2.6),
                        )),
                        2.0,
                    ),
                    m(
                        Pow,
                        Clamp::new(m(
                            Sub,
                            1.0,
                            m(Mul, Math::unary(Abs, m(Sub, "xw", 4.2)), 2.6),
                        )),
                        2.0,
                    ),
                ),
                m(
                    Max,
                    m(
                        Pow,
                        Clamp::new(m(
                            Sub,
                            1.0,
                            m(Mul, Math::unary(Abs, m(Sub, "xw", 7.8)), 2.6),
                        )),
                        2.0,
                    ),
                    m(
                        Pow,
                        Clamp::new(m(
                            Sub,
                            1.0,
                            m(Mul, Math::unary(Abs, m(Sub, "xw", 9.6)), 2.6),
                        )),
                        2.0,
                    ),
                ),
            ),
        )
        .node(
            "track_grain",
            remap(
                Noise::perlin().periods(16, 4).octaves(3).seed(1503),
                0.6,
                1.0,
            ),
        )
        .node("tracked", m(Mul, "track", "track_grain"))
        // Oil, dropped down the middle of each lane.
        .node(
            "lane_middle",
            m(
                Max,
                Clamp::new(m(
                    Sub,
                    1.0,
                    m(Mul, Math::unary(Abs, m(Sub, "xw", 3.3)), 1.8),
                )),
                Clamp::new(m(
                    Sub,
                    1.0,
                    m(Mul, Math::unary(Abs, m(Sub, "xw", 8.7)), 1.8),
                )),
            ),
        )
        .node(
            "oil",
            m(
                Mul,
                "lane_middle",
                Levels::new(Noise::perlin().periods(16, 32).octaves(3).seed(1505))
                    .in_range(0.6, 0.72),
            ),
        )
        // The repair: a fresher, darker rectangle in the far lane.
        .node(
            "patch_edge",
            remap(
                Noise::perlin().period(64).octaves(2).seed(1507),
                -0.04,
                0.04,
            ),
        )
        .node(
            "patch",
            m(
                Mul,
                band("x", 7.15, 10.1, 0.02),
                ramp(
                    m(Min, m(Sub, "y", 1.5), m(Sub, 4.4, "y")),
                    "patch_edge",
                    0.02,
                ),
            ),
        )
        .node(
            "patch_seam",
            m(
                Mul,
                Invert::new(Clamp::new(m(
                    Mul,
                    Math::unary(Abs, m(Sub, "patch", 0.5)),
                    2.2,
                ))),
                m(Step, "patch", 0.02),
            ),
        )
        // Sealant: the construction joint under the centre line, and two
        // transverse cracks, each a dark glossy band that meanders.
        .node(
            "joint_line",
            Invert::new(ramp(
                Math::unary(
                    Abs,
                    m(
                        Sub,
                        "x",
                        m(
                            Add,
                            6.08,
                            remap(
                                Noise::perlin().periods(1, 16).octaves(3).seed(1509),
                                -0.06,
                                0.06,
                            ),
                        ),
                    ),
                ),
                0.018,
                0.006,
            )),
        )
        .node(
            "crack_a",
            m(
                Mul,
                Invert::new(ramp(
                    Math::unary(
                        Abs,
                        m(
                            Sub,
                            "y",
                            m(
                                Add,
                                4.1,
                                remap(
                                    Noise::perlin().periods(16, 1).octaves(3).seed(1511),
                                    -0.12,
                                    0.12,
                                ),
                            ),
                        ),
                    ),
                    0.014,
                    0.005,
                )),
                band("x", 0.4, 5.9, 0.3),
            ),
        )
        .node(
            "crack_b",
            m(
                Mul,
                Invert::new(ramp(
                    Math::unary(
                        Abs,
                        m(
                            Sub,
                            "y",
                            m(
                                Add,
                                9.3,
                                remap(
                                    Noise::perlin().periods(16, 1).octaves(3).seed(1513),
                                    -0.1,
                                    0.1,
                                ),
                            ),
                        ),
                    ),
                    0.012,
                    0.005,
                )),
                band("x", 6.1, 11.2, 0.4),
            ),
        )
        .node(
            "sealant",
            m(
                Max,
                m(Max, "joint_line", "crack_a"),
                m(Max, "crack_b", m(Mul, "patch_seam", 0.8)),
            ),
        )
        // The paint: edge lines and the dashed centre line.
        .node(
            "edge_lines",
            m(
                Max,
                band("xw", 0.525, 0.675, 0.008),
                band("xw", 11.325, 11.475, 0.008),
            ),
        )
        .node("dash", band("dash_phase", 0.0, 0.5, 0.004))
        .node("dash_phase", Math::unary(Fract, m(Mul, "v", 2.0)))
        .node("centre_line", m(Mul, band("xw", 5.94, 6.06, 0.008), "dash"))
        .node("paint_shape", m(Max, "edge_lines", "centre_line"))
        // Where paint goes first: under tyres, over the stone tops, and in
        // blotches everywhere. `wear` sets how far it has gone.
        .node(
            "paint_wear",
            Clamp::new(m(
                Add,
                m(
                    Add,
                    m(
                        Mul,
                        Levels::new(Noise::perlin().period(128).octaves(3).seed(1515))
                            .in_range(0.3, 0.8),
                        0.6,
                    ),
                    m(Mul, "tracked", 0.5),
                ),
                m(
                    Mul,
                    Levels::new(Noise::value().period(512).seed(1517)).in_range(0.5, 1.0),
                    0.3,
                ),
            )),
        )
        .node(
            "paint",
            m(
                Mul,
                m(Mul, "paint_shape", Invert::new("patch")),
                ramp(m(Sub, 1.0, p("wear")), "paint_wear", 0.05),
            ),
        )
        // The asphalt, darkened in the paths and the oil, fresher in the
        // repair, and the sealant over it.
        .node(
            "surface_color",
            Mix::new(
                m(
                    Mul,
                    "asphalt_color",
                    m(
                        Mul,
                        m(Sub, 1.0, m(Mul, "tracked", 0.22)),
                        m(Sub, 1.0, m(Mul, "oil", 0.45)),
                    ),
                ),
                m(Mul, "asphalt_color", 0.62),
                "patch",
            ),
        )
        .node(
            "base_plane",
            Mix::new(
                "surface_color",
                [0.022, 0.022, 0.022],
                m(Mul, "sealant", Invert::new("paint_shape")),
            ),
        )
        .node(
            "paint_color",
            m(
                Mul,
                [0.7, 0.69, 0.64],
                m(
                    Mul,
                    remap(Noise::perlin().period(32).octaves(2).seed(1519), 0.82, 1.0),
                    m(Sub, 1.0, m(Mul, "tracked", 0.25)),
                ),
            ),
        )
        .node("dry_color", Mix::new("base_plane", "paint_color", "paint"))
        .node(
            "base_roughness",
            Mix::new(
                m(
                    Sub,
                    "asphalt_roughness",
                    m(Add, m(Mul, "tracked", 0.14), m(Mul, "oil", 0.3)),
                ),
                0.3,
                m(Mul, "sealant", Invert::new("paint_shape")),
            ),
        )
        .node("dry_roughness", Mix::new("base_roughness", 0.52, "paint"))
        // Height: the asphalt, the repair a millimetre down, sealant and
        // paint a little proud, the cracks where the repair is not.
        .node(
            "dry_height",
            Clamp::new(m(
                Add,
                m(Sub, "asphalt_height", m(Mul, "patch", 0.03)),
                m(Add, m(Mul, "sealant", 0.025), m(Mul, "paint_shape", 0.04)),
            )),
        )
        // Where rain stands on a carriageway: in the wheel ruts, along both
        // gutters where the camber sends it, in broad sags, and a little in
        // the texture of the surface itself.
        .node(
            "gutter",
            m(
                Max,
                Invert::new(ramp("x", 0.25, 0.5)),
                ramp("x", 11.25, 0.5),
            ),
        )
        .node(
            "pond",
            m(
                Sub,
                m(
                    Add,
                    m(
                        Mul,
                        Noise::perlin().periods(4, 8).octaves(3).seed(1521),
                        0.6,
                    ),
                    m(Mul, "dry_height", 0.5),
                ),
                m(Add, m(Mul, "tracked", 0.22), m(Mul, "gutter", 0.3)),
            ),
        );
    let g = super::surfaces::rained_on(
        g,
        ["dry_color", "dry_roughness", "dry_height"],
        "pond",
        0.45,
        0.62,
    );
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("rain.base_color")
            .roughness("rain.roughness")
            .metallic(0.0),
        [12.0, 12.0],
        0.04,
    )
}

/// Sci-fi interior panelling: 1 m panels on a 3 m wall, a dark metal kick
/// plinth, a dado seam, a frieze with a cove light, and among the panels
/// vent grilles, recessed light strips and dark display panels.
///
/// Four metres across by three up, so a storey's wall is one repeat high
/// under box UVs from the floor. The seams are recessed 8 mm in a 20 mm
/// height range. `color` scales the panel faces.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn interior_panelling() -> MaterialGraph {
    use MathOp::{Abs, Add, Floor, Fract, Max, Min, Mul, Step, Sub};
    const KICK: f32 = 0.15;
    const DADO: f32 = 1.1;
    const FRIEZE: f32 = 2.4;
    let g = MaterialGraph::builder("library:interior-panelling")
        .param(Param::color("color", [0.6, 0.61, 0.605]))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("col", Math::unary(Floor, m(Mul, "u", 4.0)))
        .node("lx", Math::unary(Fract, m(Mul, "u", 4.0)))
        .node("y", m(Mul, "v", 3.0))
        // Which band of the wall: 0 kick, 1 lower, 2 upper, 3 frieze.
        .node(
            "zone",
            m(
                Add,
                m(Step, "y", KICK),
                m(Add, m(Step, "y", DADO), m(Step, "y", FRIEZE)),
            ),
        )
        .node("panel_id", hash("col", "zone", 6.21))
        // Metres to the panel's nearest seam.
        .node("dx", m(Min, "lx", Invert::new("lx")))
        .node(
            "dy",
            m(
                Min,
                m(
                    Min,
                    Math::unary(Abs, "y"),
                    Math::unary(Abs, m(Sub, "y", KICK)),
                ),
                m(
                    Min,
                    m(
                        Min,
                        Math::unary(Abs, m(Sub, "y", DADO)),
                        Math::unary(Abs, m(Sub, "y", FRIEZE)),
                    ),
                    Math::unary(Abs, m(Sub, "y", 3.0)),
                ),
            ),
        )
        .node("edge", m(Min, "dx", "dy"))
        .node("face", ramp("edge", 0.004, 0.002))
        // A pillowed face: a centimetre chamfer into the seam.
        .node(
            "pillow",
            Clamp::new(m(Mul, m(Sub, "edge", 0.004), 1.0 / 0.012)),
        )
        // What each upper panel is.
        .node("upper", m(Mul, m(Step, "y", DADO), m(Step, FRIEZE, "y")))
        .node("vent_panel", m(Mul, "upper", m(Step, 0.2, "panel_id")))
        .node(
            "strip_panel",
            m(
                Mul,
                "upper",
                m(Mul, m(Step, "panel_id", 0.2), m(Step, 0.36, "panel_id")),
            ),
        )
        .node(
            "display_panel",
            m(
                Mul,
                "upper",
                m(Mul, m(Step, "panel_id", 0.36), m(Step, 0.46, "panel_id")),
            ),
        )
        // Vent: a grille of horizontal slots in the upper half.
        .node(
            "vent_zone",
            m(
                Mul,
                "vent_panel",
                m(
                    MathOp::Mul,
                    band("lx", 0.18, 0.82, 0.004),
                    band("y", 1.7, 2.2, 0.004),
                ),
            ),
        )
        .node("slot", band("slot_phase", 0.0, 0.55, 0.05))
        .node("slot_phase", Math::unary(Fract, m(Mul, "v", 3.0 / 0.035)))
        .node("vent", m(Mul, m(Max, "vent_zone", "low_vent"), "slot"))
        // Light strip: a recessed vertical strip up the middle.
        .node(
            "strip",
            m(
                Mul,
                "strip_panel",
                m(
                    MathOp::Mul,
                    band("lx", 0.465, 0.535, 0.003),
                    band("y", 1.25, 2.25, 0.003),
                ),
            ),
        )
        // Display: a dark screen with faint glyph rows.
        .node(
            "screen",
            m(
                Mul,
                "display_panel",
                m(
                    MathOp::Mul,
                    band("lx", 0.14, 0.86, 0.003),
                    band("y", 1.35, 1.95, 0.003),
                ),
            ),
        )
        .node(
            "glyph_cell",
            hash(
                Math::unary(Floor, m(Mul, "u", 4.0 * 40.0)),
                Math::unary(Floor, m(Mul, "v", 3.0 / 0.035)),
                7.7,
            ),
        )
        .node(
            "glyphs",
            m(
                Mul,
                "screen",
                m(
                    Mul,
                    m(Step, "glyph_cell", 0.45),
                    band("row_phase", 0.15, 0.7, 0.05),
                ),
            ),
        )
        .node("row_phase", Math::unary(Fract, m(Mul, "v", 3.0 / 0.07)))
        // Frieze cove: a slot of light in every other column.
        .node(
            "cove",
            m(
                Mul,
                m(Step, Math::unary(Fract, m(Mul, "col", 0.5)), 0.5),
                m(
                    MathOp::Mul,
                    band("y", 2.66, 2.72, 0.003),
                    band("lx", 0.06, 0.94, 0.003),
                ),
            ),
        )
        .node("kick", m(Step, KICK, "y"))
        // The frieze is ribbed: shallow horizontal grooves every 5 cm.
        .node(
            "ribs",
            m(
                Mul,
                m(Step, "y", FRIEZE + 0.03),
                m(
                    Mul,
                    Invert::new(band("y", 2.62, 2.76, 0.004)),
                    band("rib_phase", 0.0, 0.3, 0.06),
                ),
            ),
        )
        .node("rib_phase", Math::unary(Fract, m(Mul, "v", 3.0 / 0.05)))
        // A low louvred vent on a share of the lower panels.
        .node(
            "low_vent",
            m(
                Mul,
                m(Mul, m(Step, "y", KICK), m(Step, DADO, "y")),
                m(
                    Mul,
                    m(Step, 0.25, hash("col", "zone", 11.3)),
                    m(
                        MathOp::Mul,
                        band("lx", 0.25, 0.75, 0.004),
                        band("y", 0.28, 0.46, 0.004),
                    ),
                ),
            ),
        )
        // Fasteners at the corners of plain upper and lower panels.
        .node(
            "fastener",
            m(
                Mul,
                m(Mul, Invert::new("kick"), m(Step, "panel_id", 0.46)),
                Clamp::new(m(
                    Sub,
                    1.0,
                    m(
                        Mul,
                        m(
                            Max,
                            Math::unary(Abs, m(Sub, "dx", 0.04)),
                            Math::unary(Abs, m(Sub, "dy", 0.04)),
                        ),
                        1.0 / 0.007,
                    ),
                )),
            ),
        )
        // Scuffs low on the wall, and a faint hand-worn patch at hip height.
        .node(
            "scuffs",
            m(
                Mul,
                Scratches::new()
                    .count(96)
                    .length(0.02)
                    .width(0.0012)
                    .angle_spread(30.0)
                    .seed(1601),
                Clamp::new(m(Sub, 1.0, m(Mul, "y", 2.6))),
            ),
        )
        .node(
            "smudge",
            m(
                Mul,
                Levels::new(Noise::perlin().period(16).octaves(3).seed(1603)).in_range(0.5, 0.85),
                band("y", 0.8, 1.4, 0.3),
            ),
        )
        // An inset field eight centimetres inside every lower and upper
        // panel, three millimetres down, a shade lighter: the double line
        // that makes a panel read as a panel.
        .node(
            "field",
            m(
                MathOp::Mul,
                m(MathOp::Mul, Invert::new("kick"), m(Step, FRIEZE, "y")),
                ramp("edge", 0.08, 0.004),
            ),
        )
        .node(
            "field_rim",
            m(
                MathOp::Mul,
                m(MathOp::Mul, Invert::new("kick"), m(Step, FRIEZE, "y")),
                band("edge", 0.072, 0.08, 0.002),
            ),
        )
        // A bumper rail along the dado: a rounded grey strip, 6 cm, proud.
        .node(
            "rail",
            Math::unary(
                MathOp::Sqrt,
                Clamp::new(m(
                    Sub,
                    1.0,
                    m(Mul, Math::unary(Abs, m(Sub, "y", DADO)), 1.0 / 0.03),
                )),
            ),
        )
        .node("rail_mask", ramp("rail", 0.0, 0.1))
        .node(
            "panel_tone",
            m(
                Mul,
                m(
                    Mul,
                    remap("panel_id", 0.95, 1.04),
                    remap(Noise::perlin().period(8).octaves(2).seed(1605), 0.97, 1.02),
                ),
                m(
                    Sub,
                    1.0,
                    m(Add, m(Mul, "scuffs", 0.25), m(Mul, "smudge", 0.08)),
                ),
            ),
        )
        .node(
            "panel_face",
            m(
                Mul,
                m(Mul, "face", Invert::new("kick")),
                m(
                    Mul,
                    Invert::new(m(Max, "vent_zone", "low_vent")),
                    m(Mul, Invert::new("strip"), Invert::new("screen")),
                ),
            ),
        )
        .node(
            "panel_plane",
            m(
                Mul,
                m(
                    Mul,
                    "panel_tone",
                    m(
                        Sub,
                        m(
                            Add,
                            0.94,
                            m(Sub, m(Mul, "field", 0.08), m(Mul, "field_rim", 0.25)),
                        ),
                        m(Mul, "ribs", 0.18),
                    ),
                ),
                m(Mul, "panel_face", Invert::new("rail_mask")),
            ),
        )
        .node(
            "rest_plane",
            m(
                Add,
                m(
                    Add,
                    m(
                        Mul,
                        m(
                            Mul,
                            [0.13, 0.135, 0.14],
                            m(Sub, 1.0, m(Mul, "scuffs", -1.5)),
                        ),
                        m(Mul, "kick", "face"),
                    ),
                    m(
                        Add,
                        m(
                            Mul,
                            m(
                                Add,
                                [0.03, 0.032, 0.035],
                                m(Mul, [0.12, 0.12, 0.125], Invert::new("slot")),
                            ),
                            m(Max, "vent_zone", "low_vent"),
                        ),
                        m(Mul, [0.02, 0.022, 0.026], "screen"),
                    ),
                ),
                m(
                    Add,
                    m(
                        Add,
                        m(Mul, [0.7, 0.75, 0.78], "strip"),
                        m(Mul, [0.2, 0.205, 0.21], m(Mul, "rail_mask", "face")),
                    ),
                    m(Mul, [0.01, 0.01, 0.012], Invert::new("face")),
                ),
            ),
        )
        .node(
            "color_out",
            m(
                Add,
                m(Mul, p("color"), "panel_plane"),
                m(Add, "rest_plane", m(Mul, [0.25, 0.25, 0.26], "fastener")),
            ),
        )
        .node(
            "roughness",
            m(
                Add,
                m(
                    Mul,
                    "panel_face",
                    m(
                        Add,
                        0.46,
                        m(Sub, m(Mul, "smudge", -0.12), m(Mul, "scuffs", -0.1)),
                    ),
                ),
                m(
                    Add,
                    m(Mul, 0.34, m(Mul, "kick", "face")),
                    m(
                        Add,
                        m(Mul, 0.12, m(Add, "strip", "screen")),
                        m(
                            Mul,
                            0.8,
                            m(Add, m(Max, "vent_zone", "low_vent"), Invert::new("face")),
                        ),
                    ),
                ),
            ),
        )
        .node(
            "metallic",
            m(
                Add,
                m(Mul, 0.7, m(Mul, "kick", "face")),
                m(Mul, 0.8, "fastener"),
            ),
        )
        .node(
            "emissive",
            m(
                Add,
                m(Mul, [0.78, 0.9, 1.0], m(Add, "strip", "cove")),
                m(Mul, [0.12, 0.55, 0.62], "glyphs"),
            ),
        )
        .node(
            "height",
            Clamp::new(m(
                Add,
                m(
                    Add,
                    m(
                        Mul,
                        m(
                            Add,
                            m(Add, 0.3, m(Mul, "pillow", 0.4)),
                            m(Sub, m(Mul, "rail", 0.3), m(Mul, "field", 0.14)),
                        ),
                        Invert::new("kick"),
                    ),
                    m(Mul, m(Mul, "kick", "face"), 0.55),
                ),
                m(
                    Sub,
                    m(Mul, "fastener", 0.12),
                    m(
                        Add,
                        m(Add, m(Mul, "vent", 0.35), m(Mul, "ribs", 0.12)),
                        m(Mul, m(Add, "strip", "cove"), 0.2),
                    ),
                ),
            )),
        );
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic("metallic")
            .emissive("emissive"),
        [4.0, 3.0],
        0.02,
    )
}

/// A glowing advertising panel, 4 m by 2 m: one design per repeat.
///
/// A ring-and-chevron logo on the left, two rows of dot-matrix glyphs on
/// the right over an accent bar, a glowing inset border, scanlines and slow
/// brightness bands, all behind a glass cover in a dark metal frame.
/// `color` is the sign's primary colour; the accent is fixed magenta.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn holo_sign() -> MaterialGraph {
    use MathOp::{Abs, Add, Floor, Fract, Max, Min, Mul, Sin, Sqrt, Step, Sub};
    let g = MaterialGraph::builder("library:holo-sign")
        .param(Param::color("color", [0.08, 0.72, 1.0]))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("x", m(Mul, "u", 4.0))
        .node("y", m(Mul, "v", 2.0))
        // The frame: 8 cm of dark metal round the panel.
        .node(
            "edge",
            m(
                Min,
                m(Min, "x", m(Sub, 4.0, "x")),
                m(Min, "y", m(Sub, 2.0, "y")),
            ),
        )
        .node("frame", Invert::new(ramp("edge", 0.08, 0.004)))
        .node("border", band("edge", 0.12, 0.15, 0.006))
        // The logo: a ring, a chevron inside it, three bars through it.
        .node("lx", m(Sub, "x", 0.95))
        .node("ly", m(Sub, "y", 1.0))
        .node(
            "radius",
            Math::unary(Sqrt, m(Add, m(Mul, "lx", "lx"), m(Mul, "ly", "ly"))),
        )
        .node("ring", band("radius", 0.5, 0.6, 0.01))
        .node(
            "chevron",
            m(Mul, band("chev", -0.07, 0.07, 0.01), m(Step, 0.4, "radius")),
        )
        .node(
            "chev",
            m(Sub, "ly", m(Sub, 0.12, m(Mul, Math::unary(Abs, "lx"), 0.9))),
        )
        .node(
            "bars",
            m(
                Mul,
                band("lx", -0.75, 0.75, 0.01),
                m(
                    Mul,
                    band("bar_phase", 0.0, 0.3, 0.02),
                    band("ly", -0.2, 0.2, 0.01),
                ),
            ),
        )
        .node("bar_phase", Math::unary(Fract, m(Mul, "v", 2.0 / 0.14)))
        .node(
            "logo",
            m(Max, "chevron", m(Mul, "bars", m(Step, "radius", 0.62))),
        )
        // Glyphs: a 3-by-5 dot matrix per character, two rows.
        .node(
            "glyph_a",
            m(
                Mul,
                m(
                    MathOp::Mul,
                    band("x", 1.85, 3.72, 0.003),
                    band("y", 1.12, 1.58, 0.003),
                ),
                m(
                    Step,
                    0.42,
                    hash(
                        Math::unary(Floor, m(Mul, "u", 4.0 / 0.1)),
                        Math::unary(Floor, m(Mul, "v", 2.0 / 0.092)),
                        8.3,
                    ),
                ),
            ),
        )
        .node(
            "glyph_gap_a",
            m(
                Mul,
                m(Step, Math::unary(Fract, m(Mul, "u", 4.0 / 0.4)), 0.26),
                m(
                    Mul,
                    band("dot_x", 0.08, 0.92, 0.05),
                    band("dot_y", 0.08, 0.92, 0.05),
                ),
            ),
        )
        .node("dot_x", Math::unary(Fract, m(Mul, "u", 4.0 / 0.1)))
        .node("dot_y", Math::unary(Fract, m(Mul, "v", 2.0 / 0.092)))
        .node(
            "glyph_b",
            m(
                Mul,
                m(
                    MathOp::Mul,
                    band("x", 1.85, 3.3, 0.003),
                    band("y", 0.62, 0.86, 0.003),
                ),
                m(
                    Mul,
                    m(
                        Step,
                        0.45,
                        hash(
                            Math::unary(Floor, m(Mul, "u", 4.0 / 0.05)),
                            Math::unary(Floor, m(Mul, "v", 2.0 / 0.048)),
                            9.1,
                        ),
                    ),
                    m(
                        Mul,
                        m(Step, Math::unary(Fract, m(Mul, "u", 4.0 / 0.2)), 0.26),
                        m(
                            Mul,
                            band("dot_xb", 0.1, 0.9, 0.05),
                            band("dot_yb", 0.1, 0.9, 0.05),
                        ),
                    ),
                ),
            ),
        )
        .node("dot_xb", Math::unary(Fract, m(Mul, "u", 4.0 / 0.05)))
        .node("dot_yb", Math::unary(Fract, m(Mul, "v", 2.0 / 0.048)))
        .node("text", m(Max, m(Mul, "glyph_a", "glyph_gap_a"), "glyph_b"))
        // The accent bar, fading out to the right.
        .node(
            "bar",
            m(
                Mul,
                m(
                    MathOp::Mul,
                    band("x", 1.85, 3.75, 0.003),
                    band("y", 0.97, 1.03, 0.003),
                ),
                Clamp::new(m(Sub, 1.6, m(Mul, "x", 0.38))),
            ),
        )
        // Scanlines and slow brightness bands.
        .node(
            "scan",
            m(
                Add,
                0.82,
                m(Mul, 0.18, Math::unary(Sin, m(Mul, "v", 180.0))),
            ),
        )
        .node(
            "bands",
            remap(
                Noise::perlin().periods(1, 8).octaves(2).seed(1701),
                0.82,
                1.08,
            ),
        )
        .node(
            "haze",
            m(
                Mul,
                Levels::new(Noise::perlin().period(4).octaves(3).seed(1703)).in_range(0.3, 0.9),
                0.004,
            ),
        )
        .node(
            "primary",
            m(
                Mul,
                m(Add, m(Max, "ring", m(Max, "text", "border")), "haze"),
                m(Mul, "scan", "bands"),
            ),
        )
        .node(
            "accent",
            m(Mul, m(Max, "logo", "bar"), m(Mul, "scan", "bands")),
        )
        .node("screen_mask", Invert::new("frame"))
        .node(
            "emissive",
            m(
                Mul,
                "screen_mask",
                m(
                    Add,
                    m(Mul, p("color"), "primary"),
                    m(Mul, [1.0, 0.16, 0.55], "accent"),
                ),
            ),
        )
        .node(
            "frame_color",
            m(
                Mul,
                [0.09, 0.095, 0.1],
                remap(Noise::perlin().period(16).octaves(2).seed(1705), 0.85, 1.1),
            ),
        )
        .node(
            "color_out",
            Mix::new(
                m(
                    Add,
                    [0.012, 0.014, 0.018],
                    m(
                        Mul,
                        0.25,
                        m(
                            Add,
                            m(Mul, p("color"), "primary"),
                            m(Mul, [1.0, 0.16, 0.55], "accent"),
                        ),
                    ),
                ),
                "frame_color",
                "frame",
            ),
        )
        .node(
            "smears",
            Levels::new(Noise::perlin().period(8).octaves(3).seed(1707)).in_range(0.45, 0.8),
        )
        .node(
            "roughness",
            Mix::new(m(Add, 0.08, m(Mul, "smears", 0.14)), 0.42, "frame"),
        )
        .node("metallic", Mix::new(0.2, 0.8, "frame"))
        .node(
            "height",
            m(
                Add,
                0.3,
                m(
                    Mul,
                    "frame",
                    m(Add, 0.5, m(Mul, ramp("edge", 0.0, 0.02), 0.15)),
                ),
            ),
        );
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic("metallic")
            .emissive("emissive"),
        [4.0, 2.0],
        0.04,
    )
}

/// The formed concrete this graph instances, darkened: soot-black cast
/// concrete a little cool, which `color` then tints.
const STAINED_BASE: [f32; 3] = [0.024, 0.024, 0.026];

/// Dark cast concrete a storey tall, rain-streaked from the slab edge above
/// and grimed at its foot: the wall of a tower in a city that burns a lot of
/// something.
///
/// One 3.8 m storey per repeat, square, for the city grid's storeys under box
/// UVs: `v` is height up the storey, so each storey's slab edge sheds its own
/// streaks down the wall below it and the next storey starts clean at the
/// line. The substrate is `library:formed-concrete` instanced whole, laid once
/// over the repeat, so its panels are 1.9 m and its tie holes 0.95 m apart,
/// which is large-format tower formwork. Over it:
///
/// - broad soot mottle, because nothing on a dark tower is one tone;
/// - `weathering:streaks` sourced from a band just under the top of the
///   storey, gated by `streaks`, so run-off is darkest under the slab edge and
///   fades two thirds of the way down;
/// - a grime band at the foot where splash-back and street dirt sit, scaled by
///   `grime`.
///
/// `color` is the concrete's own tone; the soot is its own colour and does
/// not follow it. The whole face is matter than the formed concrete and the
/// soot matter still: at an albedo this low a dielectric's specular sheen is
/// as bright as its colour, so a glossy streak reads lighter than the wall
/// rather than darker. Relief is the formed concrete's 20 mm range, so the tie
/// holes stay 2.4 mm deep at the wider repeat.
#[must_use]
pub fn stained_concrete() -> MaterialGraph {
    use MathOp::{Div, Mul};
    let g = MaterialGraph::builder("library:stained-concrete")
        .param(Param::color("color", STAINED_BASE))
        .param(Param::float("streaks", 1.0).range(0.0, 1.0))
        .param(Param::float("grime", 0.6).range(0.0, 1.0))
        .layer(
            "concrete",
            Subgraph::new("library:formed-concrete")
                .param("color", ParamValue::Color(STAINED_BASE)),
            &CHANNELS,
        )
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("tint", m(Div, p("color"), Input::color(STAINED_BASE)))
        .node(
            "soot",
            remap(Noise::perlin().period(4).octaves(4).seed(1701), 0.72, 1.12),
        )
        .node(
            "sooted",
            m(Mul, m(Mul, "concrete.base_color", "tint"), "soot"),
        )
        // Water leaves the storey along the slab edge: a band 15 cm deep
        // just under the line.
        .node("ledge", band("v", 0.935, 0.985, 0.006))
        .layer(
            "run",
            Subgraph::new("weathering:streaks")
                .param("amount", ParamValue::Float(1.0))
                .param("opacity", ParamValue::Float(0.95))
                .param("streak_roughness", ParamValue::Float(0.92))
                .param("color", ParamValue::Color([0.006, 0.0056, 0.0052]))
                .input("base_color", "sooted")
                .input("roughness", "concrete.roughness")
                .input("source", "ledge")
                .input("bias", p("streaks")),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Extra("mask".into()),
            ],
        )
        // Splash-back and street dirt: darkest at the floor line, gone 40 cm
        // up, patchy along the wall.
        .node(
            "foot",
            m(
                Mul,
                m(
                    Mul,
                    Invert::new(Levels::new("v").in_range(0.0, 0.105).gamma(0.7)),
                    remap(
                        Noise::perlin().periods(32, 4).octaves(3).seed(1705),
                        0.4,
                        1.0,
                    ),
                ),
                p("grime"),
            ),
        )
        .node(
            "color_out",
            Mix::new(
                "run.base_color",
                m(Mul, "run.base_color", [0.3, 0.28, 0.25]),
                m(Mul, "foot", 0.8),
            ),
        )
        .node(
            "roughness",
            Mix::new(
                Clamp::new(m(MathOp::Add, "run.roughness", 0.15)),
                0.97,
                m(Mul, "foot", 0.5),
            ),
        )
        .node("height", m(Mul, "concrete.height", 1.0));
    finish_rect(
        g,
        PbrOutput::new()
            .base_color("color_out")
            .roughness("roughness")
            .metallic(0.0)
            .extra("streaks", "run.mask"),
        [3.8, 3.8],
        0.02,
    )
}

/// A window a storey tall seen from the street at night: dark reflective
/// glass over an office that may be lit, with blinds, ceiling fittings and
/// furniture against the light.
///
/// One 3.8 m storey per repeat, square, under box UVs from the storey's
/// floor, so `v` is height in the room: the lower fifth is desks and chairs in
/// silhouette, the light rises towards the ceiling, and a row of fluorescent
/// tubes runs under it. Across, the repeat is four 0.95 m cells, each with its own
/// brightness, warmth and blind, so one pane of a wide window can be half
/// covered and the next open. `blinds` is the share of cells with a blind
/// down, dropped a random depth from the head.
///
/// The emissive map is the room's light normalised to about one under the
/// fittings and a quarter at the floor; the definition's `emissive` is its
/// colour and brightness, so dim, warm and cold offices are one bake. Unlit,
/// with an `emissive` of zero, it is plain dark glazing with blinds behind it.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn office_window() -> MaterialGraph {
    use MathOp::{Add, Div, Floor, Fract, Max, Mul, Pow, Step, Sub};
    let g = MaterialGraph::builder("library:office-window")
        .param(Param::float("blinds", 0.45).range(0.0, 1.0))
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("col", Math::unary(Floor, m(Mul, "u", 4.0)))
        .node("lx", Math::unary(Fract, m(Mul, "u", 4.0)))
        // Height in the room, zero at the sill and one at the head, for a
        // window from 0.4 m to 3.4 m of the storey.
        .node("vy", Clamp::new(m(Div, m(Sub, "v", 0.1), 0.8)))
        .node("cell", hash("col", 0.0, 0.37))
        // The reflection: each cell throws back a slightly different piece of
        // sky, lighter towards its top.
        .node(
            "sky",
            remap(Noise::perlin().period(2).octaves(3).seed(1801), 0.8, 1.2),
        )
        .node(
            "reflect",
            m(
                Mul,
                m(Mul, "sky", remap("cell", 0.8, 1.2)),
                m(Add, 0.88, m(Mul, "vy", 0.22)),
            ),
        )
        .node("has_blind", m(Step, p("blinds"), hash("col", 0.0, 1.37)))
        .node("blind_drop", remap(hash("col", 0.0, 2.11), 0.15, 0.9))
        .node(
            "blind",
            m(
                Mul,
                "has_blind",
                ramp("vy", m(Sub, 1.0, "blind_drop"), 0.01),
            ),
        )
        .node(
            "slats",
            remap(Math::unary(Fract, m(Mul, "v", 76.0)), 0.72, 1.0),
        )
        // The light: a glow rising towards the ceiling, a luminaire over the
        // middle of each cell, and a cell's own level.
        .node("rise", m(Add, 0.25, m(Mul, m(Pow, "vy", 1.6), 0.6)))
        .node(
            "luminaire",
            m(
                Mul,
                band("lx", 0.08, 0.92, 0.03),
                band("vy", 0.92, 0.95, 0.006),
            ),
        )
        .node("fittings", m(Max, "rise", m(Mul, "luminaire", 1.2)))
        // Desks, chairs and screens: a row of furniture against the light
        // below desk height, in lengths of half a metre that a hash decides
        // are occupied, and the odd cold monitor over them.
        .node("block_u", Math::unary(Floor, m(Mul, "u", 8.0)))
        .node("clutter", m(Step, hash("block_u", 0.0, 5.21), 0.35))
        .node(
            "low",
            m(
                Mul,
                m(
                    Add,
                    m(Mul, band("vy", 0.0, 0.2, 0.02), m(Mul, "clutter", 0.5)),
                    m(Mul, band("vy", 0.0, 0.08, 0.02), 0.25),
                ),
                1.0,
            ),
        )
        .node(
            "screens",
            m(
                Mul,
                m(
                    Mul,
                    band("vy", 0.2, 0.26, 0.006),
                    band("lx", 0.3, 0.55, 0.02),
                ),
                m(Mul, m(Step, hash("col", 0.0, 6.47), 0.6), "clutter"),
            ),
        )
        .node(
            "glow",
            m(
                Mul,
                m(Mul, "fittings", Invert::new("low")),
                m(
                    Mul,
                    remap("cell", 0.55, 1.0),
                    Mix::new(1.0, m(Mul, "slats", 0.5), "blind"),
                ),
            ),
        )
        .node(
            "warmth",
            Mix::new([1.06, 0.98, 0.88], [0.9, 0.98, 1.08], hash("col", 0.0, 4.3)),
        )
        .node(
            "emissive",
            m(
                Add,
                m(Mul, "glow", "warmth"),
                m(Mul, "screens", [0.35, 0.6, 0.9]),
            ),
        )
        .node(
            "color",
            Mix::new(
                m(Mul, [0.024, 0.042, 0.052], "reflect"),
                m(Mul, [0.12, 0.115, 0.1], "slats"),
                m(Mul, "blind", 0.4),
            ),
        )
        .node("roughness", m(Add, 0.05, m(Mul, "cell", 0.05)));
    g.tile_metres([3.8, 3.8])
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness("roughness")
                .metallic(0.5)
                .emissive("emissive"),
        )
        .into_graph()
}

/// A lit shop seen through its window at night: shelves of goods against a
/// bright back wall, a counter in front of them, a glowing menu board and a
/// row of downlights, behind dark reflective glass.
///
/// Eight metres across by four up, under box UVs from the storey's floor, so
/// `v` is height in the shop and one repeat covers the tallest shopfront
/// pane without repeating up it. Across, the repeat is two four-metre shops,
/// each with its own counter, its own board colour and its own stock. The
/// emissive map carries the colours — goods, board, light — at about one on
/// the back wall; the definition's `emissive` is how bright the shop is, so
/// one bake serves every lit shopfront.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one graph, in field dependency order"
)]
pub fn shopfront() -> MaterialGraph {
    use MathOp::{Add, Div, Floor, Fract, Max, Mul, Pow, Step, Sub};
    let g = MaterialGraph::builder("library:shopfront")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("x", m(Mul, "u", 8.0))
        .node("y", m(Mul, "v", 4.0))
        .node("shop", Math::unary(Floor, m(Div, "x", 4.0)))
        .node("sx", m(Sub, "x", m(Mul, "shop", 4.0)))
        // The back wall: lit from above, falling off towards the floor.
        .node(
            "wall",
            m(
                Mul,
                m(
                    Add,
                    0.3,
                    m(Mul, m(Pow, Clamp::new(m(Div, "y", 3.2)), 1.4), 0.6),
                ),
                remap(hash("shop", 0.0, 1.9), 0.65, 1.0),
            ),
        )
        // Shelving to 2.1 m: a shelf every 45 cm, and on each one goods in
        // twenty-centimetre facings, each a colour and a fullness of its own.
        .node("shelving", band("y", 0.3, 2.1, 0.02))
        .node("shelf_y", m(Div, m(Sub, "y", 0.3), 0.45))
        .node("shelf", Math::unary(Floor, "shelf_y"))
        .node(
            "shelf_edge",
            Invert::new(ramp(Math::unary(Fract, "shelf_y"), 0.08, 0.04)),
        )
        .node("facing", Math::unary(Floor, m(Mul, "x", 5.0)))
        .node("goods_id", hash("facing", "shelf", 7.13))
        .node(
            "goods_color",
            Mix::new(
                Mix::new(
                    [0.9, 0.18, 0.1],
                    [0.12, 0.5, 0.9],
                    m(Step, "goods_id", 0.33),
                ),
                [0.95, 0.75, 0.2],
                m(Step, "goods_id", 0.66),
            ),
        )
        .node(
            "stocked",
            m(
                Mul,
                m(Step, 0.8, hash("facing", "shelf", 9.41)),
                ramp(Math::unary(Fract, "shelf_y"), 0.12, 0.03),
            ),
        )
        .node(
            "goods",
            m(
                Mul,
                m(
                    Mul,
                    "stocked",
                    Invert::new(ramp(Math::unary(Fract, "shelf_y"), 0.75, 0.05)),
                ),
                "shelving",
            ),
        )
        // The counter: a dark run a metre high somewhere along each shop,
        // its top edge catching the light.
        .node("counter_at", remap(hash("shop", 0.0, 3.3), 0.2, 1.8))
        .node(
            "counter_run",
            m(
                Mul,
                ramp("sx", "counter_at", 0.02),
                ramp(m(Add, "counter_at", 1.9), "sx", 0.02),
            ),
        )
        .node("counter", m(Mul, "counter_run", band("y", 0.0, 1.0, 0.01)))
        .node(
            "counter_top",
            m(Mul, "counter_run", band("y", 1.0, 1.04, 0.005)),
        )
        // The menu board over the counter: lines of lit text in the shop's
        // colour, brighter than anything else in the room.
        .node("board_run", band("sx", 0.5, 3.5, 0.02))
        .node("board", m(Mul, "board_run", band("y", 2.35, 2.8, 0.01)))
        .node(
            "text",
            m(
                Mul,
                m(
                    Step,
                    hash(
                        Math::unary(Floor, m(Mul, "x", 6.0)),
                        Math::unary(Floor, m(Mul, "y", 12.0)),
                        2.9,
                    ),
                    0.4,
                ),
                ramp(Math::unary(Fract, m(Mul, "y", 12.0)), 0.2, 0.05),
            ),
        )
        .node(
            "board_color",
            Mix::new(
                Mix::new(
                    [1.0, 0.2, 0.7],
                    [0.2, 0.9, 1.0],
                    m(Step, hash("shop", 0.0, 4.7), 0.4),
                ),
                [1.0, 0.65, 0.15],
                m(Step, hash("shop", 0.0, 4.7), 0.75),
            ),
        )
        // Downlights along the ceiling.
        .node(
            "downlights",
            m(
                Mul,
                band("y", 3.05, 3.2, 0.01),
                band_of_fract("x", 0.35, 0.65),
            ),
        )
        .node(
            "room",
            m(
                Mul,
                m(
                    Mul,
                    "wall",
                    Mix::new(
                        Mix::new(1.0, 0.55, "shelving"),
                        m(Mul, "goods_color", remap("goods_id", 0.7, 1.1)),
                        "goods",
                    ),
                ),
                m(
                    Mul,
                    Invert::new(m(Mul, "shelf_edge", m(Mul, "shelving", 0.6))),
                    Invert::new(m(Mul, "counter", 0.8)),
                ),
            ),
        )
        .node(
            "lit",
            m(
                Add,
                m(
                    Add,
                    m(Mul, "room", [1.0, 0.88, 0.72]),
                    m(Mul, "counter_top", 0.8),
                ),
                m(
                    Add,
                    m(
                        Mul,
                        m(Mul, "board", m(Add, 0.25, "text")),
                        m(Mul, "board_color", 1.6),
                    ),
                    m(Mul, "downlights", 2.0),
                ),
            ),
        )
        .node("emissive", m(Max, "lit", 0.0))
        .node(
            "reflect",
            remap(Noise::perlin().period(2).octaves(3).seed(1901), 0.8, 1.2),
        )
        .node("color", m(Mul, [0.024, 0.042, 0.052], "reflect"));
    g.tile_metres([8.0, 4.0])
        .output(
            PbrOutput::new()
                .base_color("color")
                .roughness(0.06)
                .metallic(0.5)
                .emissive("emissive"),
        )
        .into_graph()
}

/// One inside `lo..hi` of the fractional part of `x`, with a soft edge: a
/// repeating light along a run.
fn band_of_fract(x: impl Into<Input>, lo: f32, hi: f32) -> Math {
    let f = Math::unary(MathOp::Fract, x);
    m(MathOp::Mul, ramp(f.clone(), lo, 0.03), ramp(hi, f, 0.03))
}
