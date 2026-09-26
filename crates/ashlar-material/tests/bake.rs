//! What a bake writes: the planes, the normal derived from height, and the
//! bytes each map is encoded into.
//!
//! Three kinds of claim. The first is arithmetic against something computed
//! another way — an analytic gradient, the sRGB table, the IEEE half of a
//! number with a known bit pattern — so an encoder that drifts is a failing
//! number rather than a texture that looks slightly wrong later. The second is
//! shape: a level is exactly as many bytes as its format and resolution say,
//! because a length that is nearly right is a file no loader will read. The
//! third is determinism, which is the promise the design makes and the one a
//! golden hash in a later step will rest on.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::float_cmp,
    reason = "these equalities are exact by construction: a flat normal is \
              exactly [0, 0, 1], and an encoded code is an integer that \
              happens to be held in an f32; a margin here would pass a \
              normal that is not flat"
)]
#![allow(
    clippy::cast_precision_loss,
    reason = "a texel index is bounded by the resolution these tests bake at, \
              well inside what an f32 counts exactly"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::time::Duration;

use ashlar_material::{
    BlendMode, Channel, Input, MaterialGraph, MaterialGraphLibrary, MathOp, Param, ParamValue,
    PbrOutput, Period, SurfaceOutput,
    bake::{
        BakeError, BakeReport, BakeRequest, Dither, MIN_RESOLUTION, MIN_ROWS_PER_THREAD,
        PlaneFormat, Planes, bake, bake_with_report, f32_to_f16_bits, linear_to_srgb, rasterise,
    },
    nodes::{
        Blend, Bricks, Colorize, CutFlag, Decompose, GraphInput, Invert, Levels, Math, Mix, Noise,
        Subgraph, Tile, Transform, Uv,
    },
};

/// What every graph here is baked against: no subgraphs, and no overridden
/// parameters unless a test says otherwise.
struct Context {
    library: MaterialGraphLibrary,
    params: BTreeMap<String, ParamValue>,
    threads: Option<NonZeroUsize>,
}

/// Five threads rather than the default, for two reasons. Five does not divide
/// 256, so every case here rasterises four full spans and a short one and the
/// row arithmetic is under test rather than assumed. And a test binary is
/// already running its cases in parallel: the default is bounded so that a
/// small bake does not ask for the whole machine, but naming a count is what
/// makes the division of the rows the same on every machine, which is what the
/// determinism claims below rest on.
const THREADS: usize = 5;

impl Default for Context {
    fn default() -> Self {
        Self {
            library: MaterialGraphLibrary::default(),
            params: BTreeMap::new(),
            threads: NonZeroUsize::new(THREADS),
        }
    }
}

impl Context {
    fn request<'a>(&'a self, graph: &'a MaterialGraph, resolution: u32) -> BakeRequest<'a> {
        BakeRequest {
            graph,
            library: &self.library,
            params: &self.params,
            resolution,
            mips: false,
            threads: self.threads,
        }
    }
}

/// A height field of `0.5 + 0.5 * sin(2 pi p u) * sin(2 pi q v)`, which is
/// smooth, tiles at one repeat in both axes, and has a gradient that can be
/// written down.
fn sines(p: f32, q: f32, strength: f32) -> MaterialGraph {
    MaterialGraph::builder("test:sines")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
        // The graph's angles are turns, so a period of `p` is a multiply.
        .node(
            "su",
            Math::unary(MathOp::Sin, Math::new(MathOp::Mul, "u", p)),
        )
        .node(
            "sv",
            Math::unary(MathOp::Sin, Math::new(MathOp::Mul, "v", q)),
        )
        .node(
            "height",
            Math::new(
                MathOp::Add,
                Math::new(MathOp::Mul, Math::new(MathOp::Mul, "su", "sv"), 0.5),
                0.5,
            ),
        )
        .output(PbrOutput::new().height("height").normal_strength(strength))
        .into_graph()
}

/// The study concrete of the design's authoring section, with the occlusion
/// that needs a buffered filter left at its default of one: that node is phase
/// two, and everything else in the graph lowers today.
///
/// One node is here that the design's snippet does not have, and the reason
/// matters more than the node. `Blend::Add` at opacity a half is
/// `mottle + grain / 2`, which runs to 1.5, and the design's own example fed
/// that straight to `height`: a seventh of the panel came out over one, so the
/// R16 map saturated there while the normal, derived from the plane before
/// anything is clamped, still showed the relief. That is exactly the
/// disagreement between height and normal the design's bake step 4 exists to
/// rule out. The `Levels` puts the relief in `0.16..=1` so that subtracting a
/// joint of 0.16 lands the height in `0..=1` and the two maps describe one
/// surface.
fn concrete() -> MaterialGraph {
    MaterialGraph::builder("study:concrete")
        .param(Param::color("tint", [0.66, 0.65, 0.64]))
        .param(Param::float("wear", 0.0).range(0.0, 1.0).live())
        .node("coarse", Noise::value().period(4))
        .node("mottle", Noise::value().period(32))
        .node("grain", Noise::value().period(128))
        .node("pores", Levels::new("grain").in_low(0.87))
        .node(
            "rows",
            Math::new(MathOp::Mul, Decompose::new(Uv::new(), Channel::G), 8.0),
        )
        .node(
            "into_row",
            Math::new(MathOp::Sub, Math::unary(MathOp::Fract, "rows"), 0.5),
        )
        .node(
            "seams",
            Math::new(MathOp::Step, Math::unary(MathOp::Abs, "into_row"), 0.493),
        )
        .node(
            "relief",
            Blend::new(BlendMode::Add, "mottle", "grain").opacity(0.5),
        )
        .node(
            "bedded",
            Levels::new("relief")
                .in_range(0.0, 1.5)
                .out_range(0.16, 1.0),
        )
        .node(
            "height",
            Blend::new(BlendMode::Subtract, "bedded", "seams").opacity(0.16),
        )
        .node(
            "albedo",
            Colorize::new("coarse")
                .gradient([(0.0, [0.60, 0.59, 0.58]), (1.0, [0.74, 0.73, 0.71])]),
        )
        .node(
            "tinted",
            Blend::new(BlendMode::Multiply, "albedo", Input::param("tint")),
        )
        .node(
            "dirty",
            Blend::new(BlendMode::Multiply, "tinted", Invert::new("pores")),
        )
        .output(
            PbrOutput::new()
                .base_color("dirty")
                .roughness(Levels::new("coarse").out_range(0.85, 0.97))
                .metallic(0.0)
                .height("height")
                .normal_strength(0.015),
        )
        .into_graph()
}

/// Two lattices and a gradient: a base colour, a height and a normal that all
/// vary everywhere, and all of which tile at 32 repeats. Smooth on purpose —
/// the study concrete's joints are a hard step, and a map with a hard step
/// anywhere has no useful notion of a step that is too large at the seam.
fn noisy() -> MaterialGraph {
    MaterialGraph::builder("test:noisy")
        .node("coarse", Noise::value().period(8))
        .node("fine", Noise::value().period(32))
        .node(
            "mixed",
            Blend::new(BlendMode::Add, "coarse", "fine").opacity(0.4),
        )
        // `Add` at four tenths runs to 1.4; the height map holds `0..=1`.
        .node("height", Levels::new("mixed").in_range(0.0, 1.4))
        .node(
            "albedo",
            Colorize::new("height")
                .gradient([(0.0, [0.08, 0.16, 0.40]), (1.0, [0.90, 0.82, 0.55])]),
        )
        .output(
            PbrOutput::new()
                .base_color("albedo")
                .height("height")
                .normal_strength(0.02),
        )
        .into_graph()
}

/// The mean step between the two lines of an encoded map that meet at the seam,
/// and the largest mean step between neighbouring lines anywhere else.
///
/// A mean over the whole line rather than a worst texel, so one noisy texel
/// cannot decide the answer. The lines either side of the seam are left out of
/// the second number because a pass that lost the wrap writes its error into
/// exactly those, which would raise the bar by the same amount it raised the
/// seam.
fn seam_and_worst_step(
    bytes: &[u8],
    n: usize,
    stride: usize,
    channel: usize,
    across_u: bool,
) -> (f32, f32) {
    let at = |x: usize, y: usize| f32::from(bytes[(y * n + x) * stride + channel]);
    let gap = |i: usize, j: usize| {
        (0..n)
            .map(|k| {
                if across_u {
                    at(i, k) - at(j, k)
                } else {
                    at(k, i) - at(k, j)
                }
                .abs()
            })
            .sum::<f32>()
            / n as f32
    };
    let worst = (2..n - 1).map(|i| gap(i, i - 1)).fold(0.0_f32, f32::max);
    (gap(0, n - 1), worst)
}

fn planes_of(graph: &MaterialGraph, resolution: u32) -> Planes {
    let context = Context::default();
    rasterise(&context.request(graph, resolution)).unwrap().0
}

#[test]
fn a_texel_is_read_at_its_centre_rather_than_at_its_corner() {
    // Height is the u coordinate, so the plane reads back the coordinate each
    // texel was evaluated at. A corner-sampled bake would start at zero and
    // never reach the far edge, which is half a texel of skew in every map and
    // a seam that does not quite meet.
    let graph = MaterialGraph::builder("test:coordinate")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .output(PbrOutput::new().height("u").normal_strength(0.0))
        .into_graph();
    let planes = planes_of(&graph, 256);
    let height = planes.height.unwrap();
    assert!((height[0] - 0.5 / 256.0).abs() < 1e-6, "{}", height[0]);
    assert!(
        (height[255] - 255.5 / 256.0).abs() < 1e-6,
        "{}",
        height[255]
    );
    // And the next row starts over, because u does not depend on v.
    assert_eq!(height[256], height[0]);

    // The other axis, which is also what says every thread wrote the rows it
    // was given: a span that forgot its own first row would read back the
    // coordinate of some other span.
    let graph = MaterialGraph::builder("test:coordinate-v")
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .output(PbrOutput::new().height("v").normal_strength(0.0))
        .into_graph();
    let height = planes_of(&graph, 256).height.unwrap();
    for row in [0_usize, 1, 7, 8, 9, 128, 255] {
        let expected = (row as f32 + 0.5) / 256.0;
        let found = height[row * 256 + 17];
        assert!((found - expected).abs() < 1e-6, "row {row}: {found}");
    }
}

#[test]
fn a_flat_height_gives_a_flat_normal() {
    let graph = MaterialGraph::builder("test:flat")
        .output(PbrOutput::new().height(0.25).normal_strength(1.0))
        .into_graph();
    let planes = planes_of(&graph, 256);
    assert!(
        planes.normal.iter().all(|n| *n == [0.0, 0.0, 1.0]),
        "a constant height has no slope anywhere"
    );
    let set = planes.encode(Dither::Ordered);
    assert_eq!(set.normal.format, PlaneFormat::Rgba8Unorm);
    let level = &set.normal.mips[0];
    assert!(
        level
            .as_chunks::<4>()
            .0
            .iter()
            .all(|texel| *texel == [128, 128, 255, 255]),
        "flat is the middle code in both tangent axes and full in z"
    );
}

#[test]
fn a_graph_with_no_height_still_has_a_normal_map() {
    // A material may be all colour and roughness. The set always carries the
    // three required maps, so a loader never has to ask.
    let graph = MaterialGraph::builder("test:colour")
        .output(PbrOutput::new().base_color([0.2, 0.4, 0.6]))
        .into_graph();
    let planes = planes_of(&graph, 256);
    assert!(planes.height.is_none());
    assert!(planes.normal.iter().all(|n| *n == [0.0, 0.0, 1.0]));
    let set = planes.encode(Dither::None);
    assert!(set.height.is_none());
    assert!(set.emissive.is_none());
    assert_eq!(set.normal.mips.len(), 1);
}

#[test]
fn a_normal_follows_the_analytic_gradient_of_the_height_it_came_from() {
    let (periods_u, periods_v, strength) = (2.0_f32, 3.0_f32, 0.35_f32);
    let resolution = 256_u32;
    let planes = planes_of(&sines(periods_u, periods_v, strength), resolution);
    let n = resolution as usize;
    let size = f32::from(u16::try_from(resolution).unwrap());
    let tau = std::f32::consts::TAU;
    let error = |index: usize| {
        let u = ((index % n) as f32 + 0.5) / size;
        let v = ((index / n) as f32 + 0.5) / size;
        // d/du of 0.5 + 0.5 sin(tau pu u) sin(tau pv v), and the same in v.
        let du = 0.5 * tau * periods_u * (tau * periods_u * u).cos() * (tau * periods_v * v).sin();
        let dv = 0.5 * tau * periods_v * (tau * periods_u * u).sin() * (tau * periods_v * v).cos();
        let (nx, ny) = (-strength * du, -strength * dv);
        let length = (nx * nx + ny * ny + 1.0).sqrt();
        let expected = [nx / length, ny / length, 1.0 / length];
        planes.normal[index]
            .iter()
            .zip(expected)
            .fold(0.0_f32, |worst, (found, wanted)| {
                worst.max((found - wanted).abs())
            })
    };
    // A prime stride walks the whole repeat without landing on the same phase
    // twice, so the crest, the trough and the flanks are all read.
    let strided = (0..planes.normal.len()).step_by(1021);
    // And then every texel of all four edges, because the wrap in the central
    // difference is only exercised there and the stride does not reach it:
    // 1021 is 253 modulo 256, so the stride lands in column 0 and row 0 but
    // never in column 255 or row 255, and a difference that lost the right or
    // the bottom wrap would pass on the stride alone.
    let edges = (0..n).flat_map(|k| [k, (n - 1) * n + k, k * n, k * n + n - 1]);
    let (worst, at) = strided
        .chain(edges)
        .fold((0.0_f32, 0), |(worst, at), index| {
            let found = error(index);
            if found > worst {
                (found, index)
            } else {
                (worst, at)
            }
        });
    // The central difference is second-order, so at this resolution it is off
    // by about a thousandth of the slope. Anything larger is a wrong scale, a
    // wrong sign or a wrong neighbour rather than the discretisation.
    assert!(
        worst < 3e-3,
        "worst component error {worst} at ({}, {})",
        at % n,
        at / n
    );
}

#[test]
fn a_normal_leans_away_from_the_height_that_rises() {
    // The sign, on its own and in the terms the showcase's own comment uses:
    // rows run down the texture, so `+v` is downward and the green channel
    // carries `+v`. A ramp that rises toward larger u tilts the normal toward
    // smaller u, which is a negative red; a ramp rising toward larger v is a
    // negative green. Flipping either is a surface lit from the wrong side.
    let graph = MaterialGraph::builder("test:ramp")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("v", Decompose::new(Uv::new(), Channel::G))
        .node("ramp", Math::new(MathOp::Add, "u", "v"))
        .output(PbrOutput::new().height("ramp").normal_strength(0.5))
        .into_graph();
    let planes = planes_of(&graph, 256);
    // Away from the seam, where the ramp wraps, the slope is one in each axis.
    let middle = 128 * 256 + 128;
    let normal = planes.normal[middle];
    assert!(normal[0] < -0.3, "{normal:?}");
    assert!(normal[1] < -0.3, "{normal:?}");
    assert!(normal[2] > 0.0, "{normal:?}");
    assert!((normal[0] - normal[1]).abs() < 1e-5, "both axes rise alike");
}

#[test]
fn the_base_colour_goes_through_the_srgb_transfer_function() {
    // The table every 8-bit sRGB encoder is checked against: black is zero,
    // white is 255, and the half-way linear value is 188 and not 128.
    for (linear, expected) in [
        (0.0_f32, 0_u8),
        (1.0, 255),
        (0.5, 188),
        (0.25, 137),
        (0.2, 124),
        (0.001, 3),
    ] {
        let graph = MaterialGraph::builder("test:flatcolour")
            .output(PbrOutput::new().base_color([linear, linear, linear]))
            .into_graph();
        let set = planes_of(&graph, 256).encode(Dither::None);
        assert_eq!(set.base_color.format, PlaneFormat::Rgba8Srgb);
        let texel = &set.base_color.mips[0][..4];
        assert_eq!(
            texel,
            [expected, expected, expected, 255],
            "linear {linear} is sRGB {}",
            linear_to_srgb(linear)
        );
    }
}

#[test]
fn the_linear_maps_do_not_go_through_it() {
    // A roughness of 0.5 is the code 128, not the 188 the colour gets. This is
    // the mistake that makes a material look washed out and is invisible by
    // eye until it is beside a correct one.
    let graph = MaterialGraph::builder("test:linear")
        .output(
            PbrOutput::new()
                .roughness(0.5)
                .metallic(1.0)
                .occlusion(0.25),
        )
        .into_graph();
    let set = planes_of(&graph, 256).encode(Dither::None);
    assert_eq!(set.orm.format, PlaneFormat::Rgba8Unorm);
    assert_eq!(&set.orm.mips[0][..4], [64, 128, 255, 255]);
}

#[test]
fn the_ordered_dither_pays_back_what_rounding_drops() {
    // Linear 0.5 is 187.51 codes. Rounding every texel gives 188 everywhere,
    // which is a flat half-code too bright; the dither spends half the cell on
    // each side and averages to what the texel actually was.
    let graph = MaterialGraph::builder("test:dither")
        .output(PbrOutput::new().base_color([0.5, 0.5, 0.5]))
        .into_graph();
    let planes = planes_of(&graph, 256);
    let rounded = planes.encode(Dither::None);
    assert!(
        rounded.base_color.mips[0]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|texel| texel[0] == 188)
    );
    let dithered = planes.encode(Dither::Ordered);
    let cell: Vec<f32> = (0..4)
        .flat_map(|y| (0..4).map(move |x| (y * 256 + x) * 4))
        .map(|index| f32::from(dithered.base_color.mips[0][index]))
        .collect();
    assert_eq!(cell.len(), 16);
    assert!(cell.contains(&187.0), "{cell:?}");
    assert!(cell.contains(&188.0), "{cell:?}");
    let mean = cell.iter().sum::<f32>() / 16.0;
    let exact = linear_to_srgb(0.5) * 255.0;
    // A sixteen-entry cell can land the mean on any sixteenth of a code, so a
    // correct dither is within 1/32 of the exact value. The tolerance has to be
    // tighter than the half code plain rounding is off by, or rounding passes
    // this assertion too and only the codes above carry the claim.
    assert!(
        (mean - exact).abs() < 0.1,
        "cell mean {mean}, exact {exact}"
    );
}

#[test]
fn the_dither_leaves_the_map_as_tileable_as_it_found_it() {
    // The one place in the encoder that can break the crate's central promise.
    // Every plane before this is a function of UV alone and therefore wraps by
    // construction; the dither is a function of the texel index, so a cell that
    // did not divide the resolution — or an offset keyed off the flat index
    // instead of the column and row — would put a visible four-texel step down
    // the seam of an otherwise perfect tile.
    //
    // A flat colour is the strongest form of the claim: every texel wants the
    // same code, so anything that varies in the encoded plane is the dither and
    // nothing else.
    let graph = MaterialGraph::builder("test:seam")
        .output(PbrOutput::new().base_color([0.5, 0.5, 0.5]))
        .into_graph();
    for resolution in [256_u32, 512] {
        let level = &planes_of(&graph, resolution)
            .encode(Dither::Ordered)
            .base_color
            .mips[0];
        let n = resolution as usize;
        // The cell has to divide the resolution, or the columns below are
        // periodic without the seam itself being: the last cell of a row would
        // be a partial one.
        assert_eq!(n % 4, 0, "the dither cell divides {resolution}");
        let code = |x: usize, y: usize| level[(y * n + x) * 4];
        // Walking one cell along either axis is the same code, so the pattern
        // that leaves the right edge is the pattern that enters the left, and
        // the same down the bottom edge.
        for y in [0_usize, 1, 3, 7, 130, n - 1] {
            for x in [0_usize, 1, 2, 5, 9, 200, n - 4] {
                assert_eq!(
                    code(x, y),
                    code((x + 4) % n, y),
                    "at ({x}, {y}) of {resolution}, across u"
                );
                assert_eq!(
                    code(x, y),
                    code(x, (y + 4) % n),
                    "at ({x}, {y}) of {resolution}, across v"
                );
            }
        }
        // And the pattern is a pattern: a dither that answered one code
        // everywhere would satisfy everything above and dither nothing.
        let row: Vec<u8> = (0..4).map(|x| code(x, 0)).collect();
        assert!(
            row.iter().any(|byte| *byte != row[0]),
            "the cell varies along a row: {row:?}"
        );
        let column: Vec<u8> = (0..4).map(|y| code(0, y)).collect();
        assert!(
            column.iter().any(|byte| *byte != column[0]),
            "and down a column: {column:?}"
        );
    }
}

#[test]
fn a_map_that_varies_meets_itself_where_it_wraps() {
    // The tileability claim on a map that actually has something in it. The
    // dither test above bakes a flat colour, which pins the Bayer cell and
    // nothing else; this one bakes two lattices through a gradient, so every
    // texel differs from its neighbour and a field that did not meet itself —
    // a node whose periodicity rule says one thing and whose lowering does
    // another — is a hard line down the seam instead of nothing at all.
    //
    // The normal's own wrap is not read here. It is pinned exactly, at every
    // texel of all four edges, by the analytic gradient above; a mean over a
    // line is far too blunt for it, because clamping instead of wrapping
    // changes one column of derivatives and barely moves the mean.
    let set = bake(&Context::default().request(&noisy(), 256)).unwrap();
    let n = 256_usize;
    for (name, bytes, stride, channel) in [
        ("base colour red", &set.base_color.mips[0], 4, 0),
        ("base colour green", &set.base_color.mips[0], 4, 1),
        // The high byte of the R16 height, which is the same claim through the
        // other encoder.
        ("height", &set.height.as_ref().unwrap().mips[0], 2, 1),
    ] {
        for across_u in [true, false] {
            let axis = if across_u { "u" } else { "v" };
            let (seam, worst) = seam_and_worst_step(bytes, n, stride, channel, across_u);
            // Measured: the seam steps about half a code and the hardest line
            // inside steps three to four, so a field that tiled by accident
            // rather than by construction — a random jump of tens of codes —
            // is nowhere near passing this.
            assert!(
                seam <= worst,
                "{name} across {axis}: the seam steps {seam} codes, the hardest step \
                 anywhere else is {worst}"
            );
            // And the map is a map: a constant would satisfy the line above by
            // stepping zero everywhere.
            assert!(worst > 1.0, "{name} across {axis} varies: {worst}");
        }
    }
}

#[test]
fn every_encoded_level_is_exactly_the_bytes_its_format_and_resolution_say() {
    let graph = MaterialGraph::builder("test:everything")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .output(
            PbrOutput::new()
                .base_color([0.3, 0.4, 0.5])
                .roughness("u")
                .height("u")
                .emissive([1.0, 0.5, 0.25])
                .normal_strength(0.1),
        )
        .into_graph();
    let resolution = 256_u32;
    let context = Context::default();
    let set = bake(&context.request(&graph, resolution)).unwrap();
    assert_eq!(set.resolution, resolution);
    let texels = (resolution * resolution) as usize;
    let mut planes = vec![&set.base_color, &set.normal, &set.orm];
    planes.extend(set.height.as_ref());
    planes.extend(set.emissive.as_ref());
    assert_eq!(planes.len(), 5);
    for plane in planes {
        assert_eq!(
            plane.mips.len(),
            1,
            "phase one writes level 0 and nothing else"
        );
        assert_eq!(
            plane.mips[0].len(),
            texels * plane.format.bytes_per_texel(),
            "{:?}",
            plane.format
        );
    }
    let emissive = set.emissive.unwrap();
    assert_eq!(emissive.format, PlaneFormat::Rgba16Float);
    // 1.0, 0.5, 0.25 and the opaque alpha, as halves, little-endian.
    assert_eq!(
        &emissive.mips[0][..8],
        [0x00, 0x3c, 0x00, 0x38, 0x00, 0x34, 0x00, 0x3c]
    );
}

#[test]
fn a_half_carries_the_values_it_is_known_to_carry() {
    for (value, bits) in [
        (0.0_f32, 0x0000_u16),
        (-0.0, 0x8000),
        (1.0, 0x3c00),
        (-1.0, 0xbc00),
        (0.5, 0x3800),
        (2.0, 0x4000),
        (1.0 / 3.0, 0x3555),
        // Values whose dropped bits round the half up rather than away, which
        // is what tells rounding apart from truncation.
        (0.3, 0x34cd),
        (0.7, 0x399a),
        // One and a half of the smallest subnormal rounds to two, and two and
        // a half rounds to two as well: ties go to even, not upward.
        (8.940_697e-8, 0x0002),
        (1.490_116e-7, 0x0002),
        (65504.0, 0x7bff),
        // Past the largest half: saturated rather than infinite, so a texture
        // never carries an infinity a renderer would have to defend against.
        (65536.0, 0x7bff),
        (1.0e30, 0x7bff),
        // The smallest normal half, and the smallest subnormal one.
        (6.103_516e-5, 0x0400),
        (5.960_464_5e-8, 0x0001),
        // Half of the smallest subnormal rounds to even, which is zero.
        (2.980_232e-8, 0x0000),
        (1.0e-10, 0x0000),
        (f32::INFINITY, 0x7c00),
        (f32::NEG_INFINITY, 0xfc00),
    ] {
        assert_eq!(
            f32_to_f16_bits(value),
            bits,
            "{value:e} should be {bits:#06x}"
        );
    }
    // A NaN stays a NaN rather than becoming an infinity when the low mantissa
    // bits are dropped.
    let nan = f32_to_f16_bits(f32::NAN);
    assert_eq!(nan & 0x7c00, 0x7c00);
    assert_ne!(nan & 0x03ff, 0);
}

/// A bake answers zero for the mesh's cut flag, and says that it did.
///
/// Unlike a world position, which a bake refuses outright, zero *is* a picture
/// here: it is the face nobody cut, and one texture set is worn by both kinds of
/// face whatever the graph says. So the bake goes through and the report carries
/// the fact, which is what lets a content step tell an author that the reveals
/// they authored are not in the file it just wrote.
#[test]
fn a_bake_answers_zero_for_the_cut_flag_and_the_report_says_so() {
    let context = Context::default();
    let plain = MaterialGraph::builder("test:plain")
        .node("grain", Noise::value().period(4))
        .output(PbrOutput::new().base_color("grain").roughness(0.5))
        .into_graph();
    let (_, report) = bake_with_report(&context.request(&plain, MIN_RESOLUTION)).unwrap();
    assert!(!report.cut_flag, "nothing here reads the flag");

    // The same surface, plus a cut-aware half nothing on a texture can reach.
    let sawn = MaterialGraph::builder("test:sawn")
        .node("grain", Noise::value().period(4))
        .node("raw", Math::new(MathOp::Mul, "grain", 0.5))
        .node("face", Mix::new("grain", "raw", CutFlag::new()))
        .output(PbrOutput::new().base_color("face").roughness(0.5))
        .into_graph();
    let (baked, report) = bake_with_report(&context.request(&sawn, MIN_RESOLUTION)).unwrap();
    assert!(report.cut_flag, "the report says which answer it gave");

    // And the answer it gave is the uncut one, to the byte: `Op::Mix` is
    // `a + (b - a) * t`, which at zero is `a` for every finite `a`.
    let plain = bake(&context.request(&plain, MIN_RESOLUTION)).unwrap();
    assert_eq!(
        baked.base_color, plain.base_color,
        "a bake of a cut-aware graph is the graph without its cut half"
    );
}

#[test]
fn the_study_concrete_bakes_and_bakes_the_same_bytes_twice() {
    let graph = concrete();
    let context = Context::default();
    let request = context.request(&graph, 256);
    let (first, report) = bake_with_report(&request).unwrap();
    let second = bake(&request).unwrap();
    assert_eq!(first, second, "a bake is the same bytes every time");

    assert_eq!(report.resolution, 256);
    assert!(report.ops > 20, "the graph is more than a handful of ops");
    assert_eq!(
        report.ops_per_output.keys().collect::<Vec<_>>(),
        ["base_color", "height", "metallic", "occlusion", "roughness"],
        "the five ports this graph binds, and not emissive"
    );
    for (port, ops) in &report.ops_per_output {
        assert!(*ops >= 1 && *ops <= report.ops, "{port} reaches {ops}");
    }
    // Sharing is the point: the three noises feed several outputs, so the
    // per-output counts sum past the whole expression.
    let summed: usize = report.ops_per_output.values().sum();
    assert!(summed > report.ops, "{summed} against {}", report.ops);
    // Wall clock, which is what phase one's capture records.
    assert!(report.elapsed > std::time::Duration::ZERO);

    // The surface is grey concrete: nothing is black, nothing is blown out,
    // and the relief is there.
    let planes = planes_of(&graph, 256);
    assert!(
        planes
            .base_color
            .iter()
            .all(|c| c.iter().all(|channel| (0.0..=1.0).contains(channel))),
        "base colour stays in range"
    );
    let height = planes.height.unwrap();
    let low = height.iter().copied().fold(f32::INFINITY, f32::min);
    let high = height.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(high - low > 0.1, "the panel has relief: {low} to {high}");
    assert!(
        planes.normal.iter().any(|n| n[2] < 0.9999),
        "and the normal carries it"
    );
    // And the relief is relief the height map can carry. Anything outside
    // `0..=1` saturates in the R16 map while the normal, taken from the plane
    // before it is clamped, keeps the slope: the panel would read as flat where
    // it is lit as if it were not. The report says so without the caller having
    // to measure the plane.
    assert!(
        (0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high),
        "the height stays in the range its map can hold: {low} to {high}"
    );
    assert_eq!(report.height_range, Some((low, high)));
}

#[test]
fn the_same_bytes_come_out_of_any_number_of_threads() {
    // The one property the whole design of the rasteriser rests on: a texel
    // reads its own coordinate and nothing else, so which thread computed it
    // cannot matter. Two bakes on one machine only ever see one division of the
    // rows, and the golden hashes a later step records would be a golden of
    // this machine's core count if that were all this said.
    let graph = concrete();
    let mut context = Context {
        threads: NonZeroUsize::new(1),
        ..Context::default()
    };
    let (once, report) = bake_with_report(&context.request(&graph, 256)).unwrap();
    for threads in [2_usize, 3, 7, 16, 64] {
        context.threads = NonZeroUsize::new(threads);
        let (again, other) = bake_with_report(&context.request(&graph, 256)).unwrap();
        assert_eq!(once, again, "{threads} threads wrote different bytes");
        assert_eq!(report.ops, other.ops);
        assert_eq!(report.height_range, other.height_range);
    }
    // More threads than rows is not more spans than rows: a thread with no row
    // would be handed an empty slice and write nothing, which is a quarter of
    // the map missing rather than an error.
    context.threads = NonZeroUsize::new(1024);
    assert_eq!(once, bake(&context.request(&graph, 256)).unwrap());
    // And the default, whatever this machine's core count makes of it. It is
    // bounded by the work rather than by the machine — no span shorter than
    // `MIN_ROWS_PER_THREAD`, so eight spans at this resolution however many
    // cores are here — and a bound that changed the answer would be a bound
    // that lost a row.
    context.threads = None;
    assert_eq!(once, bake(&context.request(&graph, 256)).unwrap());
    assert_eq!(MIN_RESOLUTION / MIN_ROWS_PER_THREAD, 8);
}

#[test]
fn a_height_that_leaves_its_range_is_reported_rather_than_hidden() {
    // The bake will not refuse this: where a lattice reaches its extremes
    // depends on where it is sampled, so a refusal would be a graph that bakes
    // at one resolution and not at another. What it does instead is say what it
    // saw, because the map it then writes is flat exactly where the normal it
    // derived still has slope.
    let graph = MaterialGraph::builder("test:overshoot")
        .node("u", Decompose::new(Uv::new(), Channel::R))
        .node("tall", Math::new(MathOp::Mul, "u", 2.0))
        .output(PbrOutput::new().height("tall").normal_strength(0.01))
        .into_graph();
    let context = Context::default();
    let (planes, report) = rasterise(&context.request(&graph, 256)).unwrap();
    let (low, high) = report.height_range.unwrap();
    assert!((low - 2.0 * 0.5 / 256.0).abs() < 1e-6, "{low}");
    assert!((high - 2.0 * 255.5 / 256.0).abs() < 1e-6, "{high}");
    // And the map saturates over the half of the row that is past one, which
    // is what the range is there to warn about.
    let encoded = planes.encode(Dither::None);
    let height = encoded.height.unwrap();
    let last = &height.mips[0][510..512];
    assert_eq!(last, [0xff, 0xff], "the far end of the row is clamped");

    // A graph that stays inside the range is reported just the same, so a
    // caller reads one number rather than asking whether there is a problem.
    let range = bake_with_report(&context.request(&concrete(), 256))
        .unwrap()
        .1
        .height_range
        .unwrap();
    assert!(range.0 >= 0.0 && range.1 <= 1.0, "{range:?}");
}

#[test]
fn a_parameter_the_request_binds_replaces_the_one_the_graph_declares() {
    let graph = concrete();
    let mut context = Context::default();
    let plain = bake(&context.request(&graph, 256)).unwrap();
    context
        .params
        .insert("tint".to_string(), ParamValue::Color([0.2, 0.1, 0.1]));
    let tinted = bake(&context.request(&graph, 256)).unwrap();
    assert_ne!(
        plain.base_color, tinted.base_color,
        "a darker tint is a darker base colour"
    );
    assert_eq!(
        plain.height, tinted.height,
        "and nothing the tint does not reach"
    );
}

#[test]
fn a_request_is_told_what_it_asked_for_that_cannot_be_given() {
    let graph = concrete();
    let mut context = Context::default();

    assert_eq!(
        bake(&context.request(&graph, 300)).unwrap_err(),
        BakeError::Resolution(300),
        "a resolution that is not a power of two"
    );
    assert_eq!(
        bake(&context.request(&graph, 128)).unwrap_err(),
        BakeError::Resolution(128),
        "and one below the floor"
    );
    assert_eq!(
        bake(&context.request(&graph, 8192)).unwrap_err(),
        BakeError::Resolution(8192),
    );

    context
        .params
        .insert("tnit".to_string(), ParamValue::Color([0.2, 0.1, 0.1]));
    let BakeError::Graph(error) = bake(&context.request(&graph, 256)).unwrap_err() else {
        panic!("a misspelled parameter is a graph error");
    };
    assert_eq!(error.path, "params[tnit]");
    assert!(error.reason.contains("has no parameter"), "{error}");

    context.params.clear();
    context
        .params
        .insert("tint".to_string(), ParamValue::Float(0.5));
    let BakeError::Graph(error) = bake(&context.request(&graph, 256)).unwrap_err() else {
        panic!("a parameter of the wrong type is a graph error");
    };
    assert_eq!(error.path, "params[tint]");
    assert!(error.reason.contains("takes a Color"), "{error}");

    // A value the graph's own range forbids is caught by the build the
    // override feeds, at the path that build uses.
    context.params.clear();
    context
        .params
        .insert("wear".to_string(), ParamValue::Float(4.0));
    let BakeError::Graph(error) = bake(&context.request(&graph, 256)).unwrap_err() else {
        panic!("an out-of-range override is a graph error");
    };
    assert!(error.path.starts_with("params[wear]"), "{error}");
}

#[test]
fn a_lattice_finer_than_a_texel_is_refused_rather_than_sampled() {
    // A bake writes one repeat of a field, and a field whose cells are smaller
    // than the texels writing it does not come out coarser — it comes out as an
    // arbitrary sample of itself, different at every resolution and tileable
    // only by accident. The fix is more texels or a coarser graph, and both are
    // the author's to make, so the request is refused rather than warned about.
    let graph = MaterialGraph::builder("test:fine")
        .node("grain", Noise::value().period(512))
        .output(PbrOutput::new().roughness("grain"))
        .into_graph();
    let context = Context::default();
    assert_eq!(
        bake(&context.request(&graph, 256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );
    // The same lattice with the texels to carry it is ordinary.
    assert!(bake(&context.request(&graph, 512)).is_ok());

    // A noise's octaves count, because they are what a bake has to resolve: a
    // lattice of 16 doubling five times is a lattice of 512 in the picture,
    // while the period the graph reports is still the 16 the octaves sit on.
    let octaves = MaterialGraph::builder("test:octaves")
        .node("grain", Noise::value().period(16).octaves(6))
        .output(PbrOutput::new().roughness("grain"))
        .into_graph();
    assert_eq!(
        octaves.clone().build().unwrap().period(),
        Period::square(16),
    );
    assert_eq!(
        bake(&context.request(&octaves, 256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );

    // A wall counts its rows and columns rather than the bond it repeats at: a
    // running bond of 512 rows tiles in v at 256, and it is still 512 rows to
    // draw.
    let wall = MaterialGraph::builder("test:wall")
        .node("bricks", Bricks::new().rows(512).columns(4).offset(0.5))
        .output(PbrOutput::new().roughness("bricks"))
        .into_graph();
    assert_eq!(
        wall.clone().build().unwrap().period(),
        Period::Tiled { u: 4, v: 256 },
    );
    assert_eq!(
        bake(&context.request(&wall, 256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [4, 512],
        },
    );
}

#[test]
fn a_resampler_carries_the_lattice_it_read_through_what_it_did_to_the_frame() {
    // The count a bake is compared against is carried through the graph rather
    // than read off each node alone, because a resampler changes how fine a
    // lattice lands. A noise of 128 read through a scale of four lays 512
    // cells, which is four times what the noise's own node says and exactly
    // what the bake has to resolve.
    let context = Context::default();
    let scaled = MaterialGraph::builder("test:scaled")
        .node("grain", Noise::value().period(128))
        .node("closer", Transform::new("grain").scale(4.0))
        .output(PbrOutput::new().roughness("closer"))
        .into_graph();
    assert_eq!(scaled.clone().build().unwrap().finest_lattice(), [512, 512]);
    assert_eq!(
        bake(&context.request(&scaled, 256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );
    assert!(bake(&context.request(&scaled, 512)).is_ok());

    // A scatter is the same arithmetic by another name: an instance is the
    // whole source shrunk into one cell of the scatter, so sixteen of them lay
    // sixteen times what the source laid.
    let scattered = MaterialGraph::builder("test:scattered")
        .node("grain", Noise::value().period(32))
        .node("many", Tile::new("grain").count(16))
        .output(PbrOutput::new().roughness("many"))
        .into_graph();
    assert_eq!(
        scattered.clone().build().unwrap().finest_lattice(),
        [512, 512]
    );
    assert_eq!(
        bake(&context.request(&scattered, 256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );

    // And a quarter turn exchanges the axes before the scale multiplies them,
    // which is the order the period rule reads them in too. The count here is
    // the largest over the whole graph, so the 256 is the noise's own; what
    // the transform contributed is the 512 in v, which is that 256 turned
    // sideways and then doubled.
    let turned = MaterialGraph::builder("test:turned")
        .node("grain", Noise::value().periods(256, 4))
        .node(
            "sideways",
            Transform::new("grain").rotate(90.0).scales(1.0, 2.0),
        )
        .output(PbrOutput::new().roughness("sideways"))
        .into_graph();
    assert_eq!(turned.build().unwrap().finest_lattice(), [256, 512]);

    // A pointwise node lays nothing of its own and answers the finest that
    // reached it, rather than the multiple its *period* grows to: a blend of
    // four with six has a period of twelve and cells of a sixth.
    let blended = MaterialGraph::builder("test:blended")
        .node("coarse", Noise::value().period(4))
        .node("fine", Noise::value().period(6))
        .node("both", Blend::new(BlendMode::Add, "coarse", "fine"))
        .output(PbrOutput::new().roughness("both"))
        .build()
        .unwrap();
    assert_eq!(blended.period(), Period::square(12));
    assert_eq!(blended.finest_lattice(), [6, 6]);
}

#[test]
fn a_graph_counts_the_lattice_of_everything_it_instances() {
    // A subgraph is inlined at lowering, so what it lays is laid here: a bake
    // that only read the outer graph's own nodes would resolve a quarter of
    // what it wrote.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:grain")
            .node("grain", Noise::value().period(512))
            .output(PbrOutput::new().roughness("grain"))
            .into_graph(),
    );
    let outer = MaterialGraph::builder("test:outer")
        .node(
            "inner",
            Subgraph::new("test:grain").output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph();
    library.insert(outer.clone());
    assert_eq!(
        library.build("test:outer").unwrap().finest_lattice(),
        [512, 512]
    );
    let params = BTreeMap::new();
    assert_eq!(
        bake(&BakeRequest {
            graph: &outer,
            library: &library,
            params: &params,
            resolution: 256,
            mips: false,
            threads: NonZeroUsize::new(THREADS),
        })
        .unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );
}

#[test]
fn an_output_that_names_a_node_the_graph_does_not_declare_is_refused() {
    // The bake validates what it was handed rather than trusting it, because
    // the parameters it folds in are its own and can be the thing that is
    // wrong. This one is wrong before that: it reads a node nothing declares.
    let graph = MaterialGraph::builder("test:broken")
        .output(PbrOutput::new().base_color("missing"))
        .into_graph();
    let context = Context::default();
    let BakeError::Graph(error) = bake(&context.request(&graph, 256)).unwrap_err() else {
        panic!("an unknown node is a graph error");
    };
    assert_eq!(error.path, "output.base_color");
}

#[test]
fn a_graph_that_does_not_tile_is_refused_before_anything_is_rasterised() {
    // A bake writes one repeat and a renderer lays it edge to edge, so a field
    // that does not meet itself has no bake: the map would be right nowhere
    // and its seam would be visible on every wall. Adding a lattice of 2048
    // repeats to one of 3 asks for 6144, past what a bake can carry, and the
    // period of the base colour is therefore free.
    let graph = MaterialGraph::builder("test:free")
        .node("wide", Noise::value().period(2048))
        .node("odd", Noise::value().period(3))
        .node("mix", Blend::new(BlendMode::Add, "wide", "odd"))
        .output(PbrOutput::new().base_color("mix"))
        .into_graph();
    let context = Context::default();
    let BakeError::Graph(error) = bake(&context.request(&graph, 256)).unwrap_err() else {
        panic!("a free period is a graph error");
    };
    // Named at the node that ran the multiple past what a bake can carry,
    // rather than at the output that inherited it, so an author knows which
    // two lattices to bring into line.
    assert_eq!(error.path, "nodes[mix]");
    assert!(
        error.reason.contains("no common multiple"),
        "{}",
        error.reason
    );
}

#[test]
fn an_auxiliary_output_changes_nothing_a_bake_writes_or_costs() {
    // An extra is a mask a compound hands to whatever instanced it. There is
    // no map for it and no renderer slot, so a bake of the graph that declares
    // one is the bake of the same graph without it: the same five maps, and
    // the same cost, down to the instructions the expression holds. The
    // `spare` noise is what makes that a claim rather than a coincidence —
    // nothing but the extra reads it, so a bake that rooted the extras would
    // carry its hash and count it.
    let plain = MaterialGraph::builder("test:extra")
        .node("grain", Noise::value().period(8))
        .node("spare", Noise::value().period(8).seed(4))
        .output(PbrOutput::new().roughness("grain").height("grain"))
        .into_graph();
    let extra = MaterialGraph::builder("test:extra")
        .node("grain", Noise::value().period(8))
        .node("spare", Noise::value().period(8).seed(4))
        .output(
            PbrOutput::new()
                .roughness("grain")
                .height("grain")
                .extra("mask", "spare"),
        )
        .into_graph();
    let context = Context::default();
    let (bare, bare_cost) = bake_with_report(&context.request(&plain, 256)).unwrap();
    let (masked, masked_cost) = bake_with_report(&context.request(&extra, 256)).unwrap();
    assert_eq!(bare, masked);
    // Every field of the report but the clock, which is wall time and belongs
    // to the machine rather than to the graph.
    let untimed = |report: BakeReport| BakeReport {
        elapsed: Duration::ZERO,
        ..report
    };
    assert_eq!(untimed(bare_cost.clone()), untimed(masked_cost));
    // And the extra is not among the outputs that cost anything, because a
    // top-level expression has no root for it at all.
    assert_eq!(
        bare_cost.ops_per_output.keys().collect::<Vec<_>>(),
        ["base_color", "height", "metallic", "occlusion", "roughness"]
    );
}

#[test]
fn a_lattice_bound_through_an_input_is_the_lattice_the_bake_has_to_carry() {
    // The same claim as the instance above, through a signal rather than
    // through the instanced graph's own generator: the field is the outer
    // graph's, and what the compound does with it is what the bake resolves.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:pass")
            .node("field", GraphInput::float("field", 0.5))
            .output(PbrOutput::new().roughness("field"))
            .into_graph(),
    );
    let outer = MaterialGraph::builder("test:outer")
        .node("fine", Noise::value().period(512))
        .node(
            "inner",
            Subgraph::new("test:pass")
                .input("field", "fine")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph();
    library.insert(outer.clone());
    let params = BTreeMap::new();
    let request = |resolution| BakeRequest {
        graph: &outer,
        library: &library,
        params: &params,
        resolution,
        mips: false,
        threads: NonZeroUsize::new(THREADS),
    };
    assert_eq!(
        bake(&request(256)).unwrap_err(),
        BakeError::Lattice {
            resolution: 256,
            lattice: [512, 512],
        },
    );
    // And with the texels to carry it, ordinary.
    bake(&request(512)).expect("512 texels carry 512 cells");
}
