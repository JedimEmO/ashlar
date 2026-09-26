//! Surface moss that composes with any substrate and caller-authored habitat.
use crate::{
    Input, MaterialGraph, MaterialGraphBuilder,
    MathOp::{Add, Div, Mul, Sub},
    Param, PbrOutput,
    nodes::{Clamp, GraphInput, IntensityWarp, Levels, Math, Mix, Noise, Voronoi},
    stdlib::{RELIEF, bias_input, metallic_input, substrate},
};

/// Moss colonies with fine tuft relief and a shared mask for all changed channels.
///
/// Inputs: `base_color`, `roughness`, `metallic`, `height`, `bias` (where growth
/// is allowed), `shelter` (preferred habitat, e.g. mortar or hollows), and
/// `coverage` (a field multiplier on `amount`, suitable for a live control).
/// Outputs: base colour, roughness, metallic, height and extra `mask`.
/// Recompute occlusion from the resulting height in the caller. `depth` is in
/// the substrate's normalized height units. The caller sets normal strength.
/// Seeds and scales are fixed; the finest lattice is 256 for small bakes.
#[must_use]
pub fn moss() -> MaterialGraph {
    let p = Input::param;
    let g = substrate(metallic_input(bias_input(
        MaterialGraph::builder("weathering:moss")
            .param(Param::float("amount", 0.55).range(0.0, 1.0))
            .param(Param::float("softness", 0.10).range(0.01, 0.3))
            .param(Param::float("depth", 0.04).range(0.0, 0.2))
            .param(Param::color("dark_color", [0.018, 0.046, 0.003]))
            .param(Param::color("light_color", [0.200, 0.300, 0.012])),
    )))
    .node("shelter", GraphInput::float("shelter", 0.5))
    .node("coverage", GraphInput::float("coverage", 1.0))
    .node(
        "colonies",
        Noise::perlin()
            .period(4)
            .octaves(3)
            .persistence(0.6)
            .seed(81),
    )
    .node(
        "tufts",
        Noise::perlin()
            .period(64)
            .octaves(3)
            .persistence(0.65)
            .seed(82),
    )
    .node("tips", Noise::perlin().period(256).seed(83))
    .node(
        "habitat",
        Math::new(
            Add,
            Math::new(Mul, Levels::new("colonies").in_range(0.28, 0.72), 0.60),
            Math::new(
                Add,
                Math::new(Mul, Clamp::new("shelter"), 0.28),
                Math::new(Mul, "tufts", 0.12),
            ),
        ),
    )
    .node(
        "amount",
        Math::new(Mul, p("amount"), Clamp::new("coverage")),
    )
    .node(
        "mask",
        Math::new(
            Mul,
            Clamp::new("bias"),
            Clamp::new(Math::new(
                Div,
                Math::new(Sub, "habitat", Math::new(Sub, 1.0, "amount")),
                p("softness"),
            )),
        ),
    );
    let g = tuft_surface(g).output(
        PbrOutput::new()
            .base_color(Mix::new("base_color", "moss_color", "mask"))
            .roughness(Mix::new(
                "roughness",
                Levels::new("tips").out_range(0.86, 0.98),
                "mask",
            ))
            .metallic(Mix::new("metallic", 0.0, "mask"))
            .height(Mix::new("height", "raised", "mask"))
            .normal_strength(RELIEF)
            .extra("mask", "mask"),
    );
    g.into_graph()
}

// Low cushions carry clusters of upright shoots. Their cells are warped
// before profiling: regular cones read as pebbles, and smooth Perlin alone
// reads as a green sponge. The same tips drive colour and fine relief.
fn tuft_surface(g: MaterialGraphBuilder) -> MaterialGraphBuilder {
    let p = Input::param;
    g.node("cushion_cells", Voronoi::new().period(64).seed(84))
        .node(
            "cushion_warp",
            IntensityWarp::new("cushion_cells", "tufts")
                .angle(0.27)
                .amount(0.009),
        )
        .node(
            "cushions",
            Levels::new("cushion_warp").in_range(0.72, 0.06).gamma(1.0),
        )
        .node("shoot_cells", Voronoi::new().period(256).seed(85))
        .node(
            "shoot_warp",
            IntensityWarp::new("shoot_cells", "tips")
                .angle(0.61)
                .amount(0.004),
        )
        .node(
            "shoots",
            Math::new(
                Mul,
                Levels::new("shoot_warp").in_range(0.46, 0.02),
                Levels::new("tips").in_range(0.26, 0.65),
            ),
        )
        .node(
            "tone",
            Levels::new(Math::new(
                Add,
                Math::new(
                    Add,
                    Math::new(Mul, "cushions", 0.15),
                    Math::new(Mul, "tufts", 0.30),
                ),
                Math::new(
                    Add,
                    Math::new(Mul, "tips", 0.35),
                    Math::new(Mul, "shoots", 0.20),
                ),
            ))
            .in_range(0.20, 0.62),
        )
        .node(
            "moss_color",
            Math::new(
                Mul,
                Mix::new(p("dark_color"), p("light_color"), "tone"),
                Levels::new("cushions").out_range(0.58, 1.0),
            ),
        )
        .node(
            "pile",
            Math::new(
                Add,
                0.15,
                Math::new(
                    Add,
                    Math::new(Mul, "cushions", 0.45),
                    Math::new(
                        Add,
                        Math::new(Mul, "tips", 0.15),
                        Math::new(Mul, "shoots", 0.08),
                    ),
                ),
            ),
        )
        .node(
            "raised",
            Clamp::new(Math::new(Add, "height", Math::new(Mul, p("depth"), "pile"))),
        )
}
