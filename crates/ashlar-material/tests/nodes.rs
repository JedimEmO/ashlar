//! What the first set of lowered nodes computes, and that each one tiles.
//!
//! Two claims per node. The first is arithmetic: a hand-computed value at a
//! named texel, so a lowering that drifts is a failing number rather than a
//! different picture. The second is periodicity: the interpreted field meets
//! itself at both seams and repeats at the period inference claimed for it,
//! over random parameters. Tiling is the promise this crate exists to keep,
//! and it is a property of what the expression computes rather than of what
//! the node says about itself, so it is checked by evaluating.
//!
//! The one carve-out is a bare `Uv`, whose own documentation says its value
//! does not meet itself at the seam: a coordinate is one at `u = 1` and zero
//! at `u = 0`, and the period it carries is the lattice it addresses rather
//! than the value it hands out. It is tested here through something periodic
//! in it, which is what that documentation tells an author to do.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
use std::num::NonZeroUsize;

use ashlar_material::{
    BlendMode, BrickOutput, Channel, Input, Material, MaterialGraph, MaterialGraphBuilder,
    MaterialGraphLibrary, MathOp, MirrorAxis, Node, Param, ParamValue, PbrOutput, Period, SdfOp,
    ShapeKind, SurfaceOutput, Value, ValueType,
    interp::{Inputs, Interpreter, Plane, hash2},
    ir::{Ir, Target, ir_hash, lower},
    luminance,
    nodes::{
        Adjust, Blend, Blur, BlurKind, Bricks, Buffer, CircleMap, CircleSplatter, Clamp, Colorize,
        Combine, Combine2, Curvature, CurvatureKind, CurvatureOutput, Curve, CutFlag, Decompose,
        Dilate, Direction, DirectionalWarp, Distance, EdgeDetect, Erode, GradientWarp, GraphInput,
        HeightToMask, IntensityWarp, Invert, Kaleidoscope, Levels, Math, Mirror, Mix, Noise,
        NormalFromHeight, OcclusionFromHeight, Pattern, PatternKind, PatternMix, Scratches,
        SdfCombine, SdfMask, Shape, ShapeOutput, Subgraph, Switch, Tile, TilePattern, Tiles, Time,
        Transform, Triplanar, Uv, Voronoi, VoronoiMetric, VoronoiOutput, Warp, Weave, WeaveOutput,
        WeavePattern, WorldMask, WorldNormal, WorldPos,
    },
    planes::{BakeCache, rasterise_buffers},
};
use proptest::{prelude::*, test_runner::TestCaseError};

/// Six decimals, which is the last place an `f32` expression this short holds.
///
/// Identical bits count as close whatever they are, because a graph is allowed
/// to write `log2(0)`: two infinities are the same value at both seams, and
/// subtracting one from the other would call them different.
fn close(left: f32, right: f32) -> bool {
    left.to_bits() == right.to_bits() || (left - right).abs() < 1e-6
}

/// The same, across three lanes.
fn close3(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter().zip(right).all(|(l, r)| close(*l, r))
}

/// A material lowered for the bake, with the texel's two axes already
/// decomposed as `u` and `v` for whatever the test wires to them.
fn wired(build: impl FnOnce(MaterialGraphBuilder) -> MaterialGraphBuilder) -> Ir {
    let builder = MaterialGraph::builder("test:node")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G));
    let material = build(builder).build().unwrap();
    lower(&material, Target::Bake).unwrap()
}

/// One node under test, read through the roughness output.
fn float_node(node: impl Into<Node>) -> Ir {
    wired(|builder| {
        builder
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
    })
}

/// The value of one output port at one texel.
fn at(ir: &Ir, port: &str, uv: [f32; 2]) -> [f32; 3] {
    let root = ir.root(port).expect("a bound output");
    Interpreter::new(ir)
        .eval(uv, &Inputs::default(), root)
        .unwrap()
}

/// The roughness at one texel, which is where a float node is read.
fn float_at(ir: &Ir, uv: [f32; 2]) -> f32 {
    at(ir, "roughness", uv)[0]
}

/// The base colour at one texel, which is where a colour node is read.
fn colour_at(ir: &Ir, uv: [f32; 2]) -> [f32; 3] {
    at(ir, "base_color", uv)
}

/// A lowered material and the planes its buffered filters left behind.
///
/// A pointwise node is a function of its texel and needs nothing else; a
/// buffered one is a function of a plane, and a plane exists only once a bake
/// has rasterised it. So the properties below read a field through this, and a
/// graph with nothing buffered in it carries no planes and is the expression
/// evaluated directly.
struct Field {
    ir: Ir,
    planes: Vec<Plane>,
}

/// The smallest resolution a bake allows, which is what the buffered cases are
/// rasterised at: a jump flood over a megatexel per case is not a unit test.
const SMALL: u32 = 256;

/// Two threads, named rather than defaulted, because a property test already
/// runs its cases against a machine that is busy.
fn threads() -> Option<NonZeroUsize> {
    NonZeroUsize::new(2)
}

impl Field {
    /// Lower a material, rasterising its planes where it has any.
    fn of(material: &Material) -> Self {
        let ir = lower(material, Target::Bake).unwrap();
        let planes = rasterise_buffers(&ir, SMALL, threads(), &mut BakeCache::new()).unwrap();
        Self { ir, planes }
    }

    /// The base colour at one texel.
    fn colour_at(&self, uv: [f32; 2]) -> [f32; 3] {
        let root = self.ir.root("base_color").expect("a bound output");
        let inputs = Inputs {
            buffers: &self.planes,
            ..Inputs::default()
        };
        Interpreter::new(&self.ir).eval(uv, &inputs, root).unwrap()
    }
}

/// The seam claim: the texel at `u = 0` and the one at `u = 1` are the same
/// texel of the next repeat along, in both axes.
fn meets_itself(label: &str, field: &Field, drawn: f32) -> Result<(), TestCaseError> {
    for along in [0.0_f32, 0.137, 0.5, 0.813, 0.999, drawn] {
        let west = field.colour_at([0.0, along]);
        let east = field.colour_at([1.0, along]);
        prop_assert!(
            tiles(west, east),
            "{label} at v = {along}: {west:?} at u = 0 and {east:?} at u = 1",
        );
        let south = field.colour_at([along, 0.0]);
        let north = field.colour_at([along, 1.0]);
        prop_assert!(
            tiles(south, north),
            "{label} at u = {along}: {south:?} at v = 0 and {north:?} at v = 1",
        );
    }
    Ok(())
}

/// The repeat claim: a texel and the same texel a whole repeat along carry the
/// same value, across the plane rather than only at the seam.
fn repeats(label: &str, field: &Field, drawn: f32, across: f32) -> Result<(), TestCaseError> {
    // The grid walks the unit; the drawn pair lands anywhere in it, so a rule
    // that happens to hold on eighths has nowhere to hide.
    let walk = (0_u16..8).map(|step| f32::from(step) / 8.0 + 1.0 / 64.0);
    for coordinate in walk.chain([drawn]) {
        for other in [0.0_f32, 0.375, 0.625, across] {
            let here = field.colour_at([coordinate, other]);
            let next = field.colour_at([coordinate + 1.0, other]);
            prop_assert!(
                tiles(here, next),
                "{label} at u = {coordinate} is {here:?}, and one repeat \
                 along it is {next:?}",
            );
            let here = field.colour_at([other, coordinate]);
            let next = field.colour_at([other, coordinate + 1.0]);
            prop_assert!(
                tiles(here, next),
                "{label} at v = {coordinate} is {here:?}, and one repeat \
                 along it is {next:?}",
            );
        }
    }
    Ok(())
}

#[test]
fn a_coordinate_decomposes_into_its_axes_and_combines_back() {
    let ir = wired(|builder| {
        builder
            .node("n", Combine::new("u", "v", 0.5))
            .output(PbrOutput::new().base_color("n"))
    });
    let colour = colour_at(&ir, [0.25, 0.75]);
    assert!(close3(colour, [0.25, 0.75, 0.5]), "{colour:?}");

    // And the lane a decompose names is the lane it reads back.
    let ir = wired(|builder| {
        builder
            .node("c", Combine::new("u", "v", 0.5))
            .node("n", Decompose::new("c", Channel::B))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&ir, [0.25, 0.75]), 0.5));

    // A float arrived broadcast, so every channel of it is the float itself.
    let ir = float_node(Decompose::new("u", Channel::G));
    assert!(close(float_at(&ir, [0.3, 0.7]), 0.3));

    // A bare coordinate is the one field in this set that does not meet itself
    // at the seam, and its documentation says so: it is one at `u = 1` and zero
    // at `u = 0`. The periodicity properties below therefore reach it through a
    // turn of sine, so the discontinuity is pinned here rather than assumed.
    let ir = float_node(Decompose::new("uv", Channel::R));
    assert!(close(float_at(&ir, [0.0, 0.5]), 0.0));
    assert!(close(float_at(&ir, [1.0, 0.5]), 1.0));
}

/// A `Vec2` node read through the two lanes of a colour, with whatever planes
/// it wanted rasterised first.
///
/// [`float_node`] reads a node through the roughness, which is one channel and
/// takes no `Vec2` at all: a `Vec2` converts to nothing, which is the graph's
/// own rule. So the two lanes are decomposed and recombined into a colour,
/// which is what a test of an orientation field can actually look at.
fn vec2_at(node: impl Into<Node>, uv: [f32; 2]) -> [f32; 2] {
    let material = MaterialGraph::builder("test:vec2")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
        .node("n", node)
        .output(PbrOutput::new().base_color(Combine::new(
            Decompose::new("n", Channel::R),
            Decompose::new("n", Channel::G),
            0.0,
        )))
        .build()
        .unwrap();
    let lanes = Field::of(&material).colour_at(uv);
    [lanes[0], lanes[1]]
}

#[test]
fn two_floats_combine_into_the_displacement_a_vec2_port_takes() {
    let axes = vec2_at(Combine2::new("u", "v"), [0.25, 0.75]);
    assert!(close(axes[0], 0.25) && close(axes[1], 0.75), "{axes:?}");
    // A literal on either side is a literal, folded where it stands.
    let half = vec2_at(Combine2::new(0.5, "v"), [0.25, 0.75]);
    assert!(close(half[0], 0.5) && close(half[1], 0.75), "{half:?}");
}

#[test]
fn a_direction_is_the_unit_circle_at_an_angle_and_the_uphill_gradient_of_a_slope() {
    // Turns rather than radians, as every other angle the graph carries is: a
    // quarter is a quarter turn without anybody converting anything.
    for (turns, expected) in [
        (0.0_f32, [1.0_f32, 0.0_f32]),
        (0.25, [0.0, 1.0]),
        (0.5, [-1.0, 0.0]),
        (0.75, [0.0, -1.0]),
    ] {
        let answer = vec2_at(Direction::from_angle("u"), [turns, 0.5]);
        assert!(
            (answer[0] - expected[0]).abs() < 1e-5 && (answer[1] - expected[1]).abs() < 1e-5,
            "{turns} turns answered {answer:?} rather than {expected:?}"
        );
    }

    // The quarter turn is exact — `(x, y) -> (-y, x)` rather than a rotation
    // through a folded sine — so an angle of zero turns to exactly `(0, 1)`.
    let turned = vec2_at(Direction::from_angle(0.0).rotate_quarter(), [0.3, 0.7]);
    assert!(close(turned[0], 0.0) && close(turned[1], 1.0), "{turned:?}");

    // A height that rises along u has its gradient along `+u`, at unit length
    // whatever the slope, because a direction is a direction.
    let slope = vec2_at(Direction::from_slope("u"), [0.5, 0.5]);
    assert!(
        close(slope[0], 1.0) && close(slope[1], 0.0),
        "an uphill gradient along u: {slope:?}"
    );
    let along = vec2_at(Direction::from_slope("u").rotate_quarter(), [0.5, 0.5]);
    assert!(
        close(along[0], 0.0) && close(along[1], 1.0),
        "and along its contours: {along:?}"
    );
    let downhill = vec2_at(Direction::from_slope(Invert::new("u")), [0.5, 0.5]);
    assert!(close(downhill[0], -1.0), "the other way: {downhill:?}");

    // Flat ground has no gradient, and the honest answer to "which way" there
    // is none at all rather than an arbitrary axis.
    let flat = vec2_at(Direction::from_slope(0.5), [0.5, 0.5]);
    assert!(close(flat[0], 0.0) && close(flat[1], 0.0), "{flat:?}");
}

#[test]
fn a_value_noise_is_the_corner_hash_where_the_weights_vanish() {
    // On a lattice corner both smoothed weights are zero, so the bilinear
    // collapses to that corner's own hash and the node is its hash table.
    let seeded = float_node(Noise::value().period(4).seed(3));
    for (uv, cell) in [
        ([0.0_f32, 0.0_f32], (0_u32, 0_u32)),
        ([0.25, 0.5], (1, 2)),
        ([0.75, 0.75], (3, 3)),
    ] {
        let expected = hash2(cell.0, cell.1, 3);
        let value = float_at(&seeded, uv);
        assert!(close(value, expected), "at {uv:?}: {value} not {expected}");
    }

    // The lattice wraps at the period, so the cell past the last one is the
    // first: this is the same corner read from the other side of the seam.
    assert!(close(float_at(&seeded, [1.0, 0.0]), hash2(0, 0, 3)));

    // A seed moves the field without moving the lattice.
    let plain = float_node(Noise::value().period(4));
    assert!(!close(
        float_at(&plain, [0.25, 0.5]),
        float_at(&seeded, [0.25, 0.5])
    ));
    assert!(close(float_at(&plain, [0.25, 0.5]), hash2(1, 2, 0)));

    // Between corners it is the smoothstep-weighted bilinear, which at the
    // centre of a cell weighs each of the four corners a quarter.
    let middle = float_at(&plain, [0.125, 0.125]);
    let corners: f32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
        .into_iter()
        .map(|(x, y)| hash2(x, y, 0) * 0.25)
        .sum();
    assert!(close(middle, corners), "{middle} not {corners}");
}

#[test]
fn octaves_sum_at_a_multiplied_period_and_a_diminished_amplitude() {
    // Two octaves are the coarse lattice plus half of one twice as fine, on
    // the next seed, divided by the one and a half they carry between them.
    let coarse = float_node(Noise::value().period(4));
    let fine = float_node(Noise::value().period(8).seed(1));
    let both = float_node(
        Noise::value()
            .period(4)
            .octaves(2)
            .lacunarity(2)
            .persistence(0.5),
    );
    for uv in [[0.1_f32, 0.2_f32], [0.43, 0.77], [0.0, 0.0]] {
        let expected = (float_at(&coarse, uv) + 0.5 * float_at(&fine, uv)) / 1.5;
        let value = float_at(&both, uv);
        assert!(close(value, expected), "at {uv:?}: {value} not {expected}");
    }

    // One octave is the octave itself: no sum, and nothing divided.
    let one = float_node(Noise::value().period(4).octaves(1).persistence(0.25));
    assert!(close(
        float_at(&one, [0.31, 0.62]),
        float_at(&coarse, [0.31, 0.62])
    ));
}

#[test]
fn a_brick_mask_is_one_on_the_face_and_zero_in_the_mortar() {
    // Four rows of two at a running bond, with a tenth of a brick of mortar
    // on every side.
    let wall = float_node(Bricks::new().rows(4).columns(2).offset(0.5).mortar(0.1));
    // The middle of a brick in row zero: half a brick along, half a brick up.
    assert!(close(float_at(&wall, [0.25, 0.125]), 1.0));
    // A row boundary is mortar, at a u that is well inside a brick face so
    // that it is the row that answers and not the column.
    assert!(close(float_at(&wall, [0.37, 0.125]), 1.0));
    assert!(close(float_at(&wall, [0.37, 0.25]), 0.0));
    // So is a column boundary: a wall of bricks has vertical joints too, and a
    // graph that wants the row seams alone builds its band from `Uv` and
    // `Math` rather than narrowing the wall to one column.
    assert!(close(float_at(&wall, [0.0, 0.125]), 0.0));
    assert!(close(float_at(&wall, [0.5, 0.125]), 0.0));

    // The bond is the whole of the period in v: row one is laid half a brick
    // along, so what is mortar in row zero is face in row one.
    assert!(close(float_at(&wall, [0.0, 0.375]), 1.0));
    assert!(close(float_at(&wall, [0.25, 0.375]), 0.0));

    // The edge belongs to the face: a texel exactly on a mortar boundary reads
    // one, so the two bands are closed intervals. At row zero the brick-local
    // coordinate is `fract(2u)`, and both of these land on a boundary exactly:
    // halving and doubling are exact in binary, and `1 - mortar` rounds to the
    // same `f32` the other one reaches.
    assert!(close(float_at(&wall, [0.05, 0.125]), 1.0), "the near edge");
    assert!(close(float_at(&wall, [0.45, 0.125]), 1.0), "the far edge");

    // Narrowing a wall to one column does not turn it into a band in v. The
    // mortar comes off the u sides too, so one joint per row still runs down
    // the wall, moved along by the bond: this is the groove a cast panel does
    // not have, and it is why the study's seam is built from `Uv` and `Math`.
    let panel = float_node(Bricks::new().rows(8).columns(1).mortar(0.007));
    assert!(
        close(float_at(&panel, [0.25, 0.30]), 1.0),
        "the face of an even row"
    );
    assert!(
        close(float_at(&panel, [0.0, 0.30]), 0.0),
        "its joint, at u = 0"
    );
    assert!(
        close(float_at(&panel, [0.5, 0.18]), 0.0),
        "and half a brick along, an odd row"
    );

    // Without mortar there is nothing but face.
    let solid = float_node(Bricks::new().rows(4).columns(2).mortar(0.0));
    for uv in [[0.0_f32, 0.0_f32], [0.5, 0.25], [0.123, 0.456]] {
        assert!(close(float_at(&solid, uv), 1.0), "at {uv:?}");
    }
}

#[test]
fn the_settings_this_backend_cannot_lower_are_refused_by_path() {
    // Every node kind lowers now, and so does every output of every one of
    // them. What is left to refuse is a handful of *settings*, and what holds
    // here is that asking for one is a named refusal at the node that asked
    // rather than a picture that is quietly the rectangle, or the value noise,
    // instead.
    let refused = |node: Node| {
        let material = MaterialGraph::builder("test:unlowered")
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
            .build()
            .unwrap();
        let error = lower(&material, Target::Bake).unwrap_err();
        assert_eq!(error.path, "nodes[n]");
        error.reason
    };
    // The face a wall cuts is a rectangle, and a wall that asked for a rounded
    // or cut corner would otherwise get the rectangle with nothing to read the
    // difference off.
    assert_eq!(
        refused(Bricks::new().round(0.2).into()),
        "rounded brick corners have no lowering yet"
    );
    assert_eq!(
        refused(Bricks::new().corner(0.2).into()),
        "cut brick corners have no lowering yet"
    );
    // A simplex noise is the one refusal in the crate that is not a backend
    // waiting its turn: its lattice has no integer period at all, so the reason
    // says why rather than promising one later.
    let simplex = refused(Noise::simplex().period(8).into());
    assert!(
        simplex.contains("no integer period") && !simplex.contains("yet"),
        "{simplex}"
    );
    // Two of the settings that used to be on this list are gone, and their
    // absence is the claim now: a directional blur and a slope blur lower, and
    // each is checked for what it computes in `tests/buffers.rs`.
    for kind in [BlurKind::Directional, BlurKind::Slope] {
        let material = MaterialGraph::builder("test:blurs")
            .node("grain", Noise::value().period(8))
            .node("n", Blur::new("grain").kind(kind).radius(0.02))
            .output(PbrOutput::new().roughness("n"))
            .build()
            .unwrap();
        assert!(lower(&material, Target::Bake).is_ok(), "{kind:?}");
    }

    // Every output of a wall lowers, which is what the list above used to
    // hold: they are checked for what they compute further down.
    for output in [
        BrickOutput::Mask,
        BrickOutput::Bevel,
        BrickOutput::Id,
        BrickOutput::Fill,
    ] {
        let wall = float_node(Bricks::new().output(output).mortar(0.1));
        assert!(float_at(&wall, [0.25, 0.125]).is_finite(), "{output:?}");
    }
}

#[test]
fn a_wall_meets_itself_a_bond_along_which_is_what_its_period_in_v_names() {
    // Brickwork is the case where a period really is a shift the field is
    // invariant under, because every brick is a copy of every other: a wall of
    // eight rows at a running bond is the same wall two rows up, which is the
    // `2x4` that inference names. A lattice generator claims no such thing —
    // the eight cells of a noise of period eight hash differently on purpose,
    // and no node may lean on the finer shift — so this is checked here rather
    // than over the whole vocabulary.
    let material = MaterialGraph::builder("test:wall")
        .node(
            "n",
            Bricks::new().rows(8).columns(2).offset(0.5).mortar(0.1),
        )
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    assert_eq!(
        material.port("n").unwrap().period,
        Period::Tiled { u: 2, v: 4 }
    );
    let ir = lower(&material, Target::Bake).unwrap();
    // A brick-local coordinate well inside the face: the mask has a hard edge
    // at the mortar, and a texel sitting exactly on one is decided by the last
    // bit of a coordinate two rows apart accumulated differently, which is a
    // question about `f32` rather than about the bond.
    let inside = 0.37;
    for step in 0_u16..64 {
        let t = f32::from(step) / 64.0;
        let (here, along) = (float_at(&ir, [t, 0.3]), float_at(&ir, [t + 0.5, 0.3]));
        assert!(close(here, along), "a column along, at u = {t}");
        let (here, up) = (
            float_at(&ir, [inside, t]),
            float_at(&ir, [inside, t + 0.25]),
        );
        assert!(close(here, up), "a bond up, at v = {t}");
    }

    // And it is not the same wall one row up, which is the over-claim the
    // bond rule exists to refuse: row two is laid where row three is not.
    assert!(!close(
        float_at(&ir, [0.0, 0.3]),
        float_at(&ir, [0.0, 0.425])
    ));

    // A stack bond has no shift at all, so there every row is every other.
    let material = MaterialGraph::builder("test:stack")
        .node(
            "n",
            Bricks::new().rows(8).columns(2).offset(0.0).mortar(0.1),
        )
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    assert_eq!(
        material.port("n").unwrap().period,
        Period::Tiled { u: 2, v: 8 }
    );
    let ir = lower(&material, Target::Bake).unwrap();
    for step in 0_u16..64 {
        let t = f32::from(step) / 64.0;
        let (here, up) = (
            float_at(&ir, [inside, t]),
            float_at(&ir, [inside, t + 0.125]),
        );
        assert!(close(here, up), "a row up, at v = {t}");
    }
}

#[test]
fn a_wall_whose_bond_drifts_across_the_rows_is_refused_rather_than_baked() {
    // A bond of a ten-thousandth of a brick reads like a stack bond one row at
    // a time, and is not one: the shift accumulates, so over four thousand
    // rows the wall walks four tenths of a brick along and the row under the v
    // seam is laid nowhere near row zero. The bond rule is about the whole
    // wall for this reason, and the wall is refused at the field that decides
    // it rather than baked with a period it does not have.
    let error = MaterialGraph::builder("test:drift")
        .node(
            "n",
            Bricks::new()
                .rows(4096)
                .columns(1)
                .offset(0.0001)
                .mortar(0.05),
        )
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].offset");

    // And this is what the refusal is worth, because a bound no picture needs
    // is only pedantry: the mask that wall would have cut, written out of `Uv`
    // and `Math` so that it can be evaluated at all, is mortar at `v = 0` and
    // face at `v = 1` over the same column. A wall that validated here would
    // have baked that line into every texture it tiled.
    let drifting = wired(|builder| {
        builder
            .node(
                "row",
                Math::unary(MathOp::Floor, Math::new(MathOp::Mul, "v", 4096.0)),
            )
            .node("shift", Math::new(MathOp::Mul, "row", 0.0001))
            .node(
                "along",
                Math::unary(MathOp::Fract, Math::new(MathOp::Add, "u", "shift")),
            )
            .node("n", Math::new(MathOp::Step, "along", 0.05))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&drifting, [0.0, 0.0]), 0.0), "the joint");
    assert!(
        close(float_at(&drifting, [0.0, 1.0]), 1.0),
        "and where the joint has walked to by the seam"
    );
}

#[test]
fn the_study_panel_seam_is_a_band_in_v_that_nothing_crosses() {
    // The study's pixel loop dropped the height where
    // `((v * 8).fract() - 0.5).abs() > 0.493`: eight rows of panel with a joint
    // across each edge and nothing running down the wall. That is `Uv` through
    // `Math` and not `Bricks`, whose mortar comes off all four sides, and it is
    // the graph the README and the crate documentation show for it.
    let ir = wired(|builder| {
        builder
            .node("rows", Math::new(MathOp::Mul, "v", 8.0))
            .node(
                "into_row",
                Math::new(MathOp::Sub, Math::unary(MathOp::Fract, "rows"), 0.5),
            )
            .node(
                "n",
                Math::new(MathOp::Step, Math::unary(MathOp::Abs, "into_row"), 0.493),
            )
            .output(PbrOutput::new().roughness("n"))
    });
    // Read across the whole width as well as down, because the thing this band
    // has over a wall of one column is that no u is special: a groove down the
    // wall would show as a column that is mortar at every v.
    for u in [0.0_f32, 0.25, 0.5, 0.937_5] {
        for step in 0_u16..4096 {
            let v = f32::from(step) / 4096.0;
            let seam = ((v * 8.0).fract() - 0.5).abs();
            let expected = if seam > 0.493 { 1.0 } else { 0.0 };
            let value = float_at(&ir, [u, v]);
            assert!(
                close(value, expected),
                "u = {u}, v = {v}, seam = {seam}: {value}"
            );
        }
    }
}

#[test]
fn levels_maps_the_input_range_onto_the_output_range_through_a_gamma() {
    let ir = float_node(
        Levels::new("u")
            .in_range(0.25, 0.75)
            .gamma(2.0)
            .out_range(0.0, 1.0),
    );
    // Halfway through the input range, squared.
    assert!(close(float_at(&ir, [0.5, 0.0]), 0.25));
    // Outside it the ends hold, which is what makes this a remap.
    assert!(close(float_at(&ir, [0.1, 0.0]), 0.0));
    assert!(close(float_at(&ir, [0.9, 0.0]), 1.0));

    // An output range that starts high and ends low runs backwards.
    let flipped = float_node(Levels::new("u").out_range(1.0, 0.0));
    assert!(close(float_at(&flipped, [0.25, 0.0]), 0.75));

    // The two channel modes are told apart by a curve that is not a plain
    // scale, because a plain scale is the one case where they agree: the same
    // colour `(0.9, 0.1, 0.5)` through the same range and gamma.
    let curved = |on_luminance: bool| {
        let ir = wired(|builder| {
            let mut levels = Levels::new("c").in_range(0.2, 0.8).gamma(2.0);
            if on_luminance {
                levels = levels.luminance();
            }
            builder
                .node("c", Combine::new("u", 0.1, 0.5))
                .node("n", levels)
                .output(PbrOutput::new().base_color("n"))
        });
        colour_at(&ir, [0.9, 0.0])
    };

    // Per channel each lane meets the curve alone: 0.9 is past the top of the
    // input range and 0.1 below its bottom, so both clamp to an end, and only
    // the middle lane is halfway up and squared.
    let per_channel = curved(false);
    assert!(close3(per_channel, [1.0, 0.0, 0.25]), "{per_channel:?}");

    // On luminance the curve moves the brightness and the colour follows it by
    // the ratio, so nothing clips and the hue is where it was: a brightness of
    // 0.298_96 is curved to 0.027_203, a ratio of 0.090_992 that every lane is
    // scaled by. A lowering that ignored the mode would answer the three
    // numbers above instead.
    let on_luminance = curved(true);
    assert!(
        close3(on_luminance, [0.081_892_9, 0.009_099_2, 0.045_496_1]),
        "{on_luminance:?}"
    );
    // And the ratio really is a scale: the hue the colour arrived with survives.
    assert!(
        close(on_luminance[0] / on_luminance[1], 9.0),
        "{on_luminance:?}"
    );
}

#[test]
fn a_blend_is_the_mode_over_the_backdrop_by_the_opacity() {
    // `u` is the backdrop at 0.8, and 0.2 goes over it at half opacity.
    for (mode, expected) in [
        (BlendMode::Normal, 0.5),
        (BlendMode::Multiply, 0.48),
        (BlendMode::Add, 0.9),
        (BlendMode::Subtract, 0.7),
    ] {
        let ir = float_node(Blend::new(mode, "u", 0.2).opacity(0.5));
        let value = float_at(&ir, [0.8, 0.0]);
        assert!(close(value, expected), "{mode:?} answered {value}");
    }

    // No opacity is the backdrop untouched, whatever the mode did.
    let ir = float_node(Blend::new(BlendMode::Multiply, "u", 0.2).opacity(0.0));
    assert!(close(float_at(&ir, [0.8, 0.0]), 0.8));

    // A mask can drive the opacity, so it is an input and not a number.
    let ir = float_node(Blend::new(BlendMode::Normal, 0.0, 1.0).opacity("u"));
    assert!(close(float_at(&ir, [0.3, 0.0]), 0.3));

    // The rest of the modes, over a backdrop of 0.8 and a 0.2 laid on it at
    // full opacity, so that what is read is the mode itself.
    for (mode, expected) in [
        // One less the product of the two complements.
        (BlendMode::Screen, 0.84),
        // The backdrop is light, so this is the screen half of the mode.
        (BlendMode::Overlay, 0.68),
        (BlendMode::Difference, 0.6),
        (BlendMode::Lighten, 0.8),
        (BlendMode::Darken, 0.2),
        // Pegtop: `(1 - 2b) a^2 + 2ab`.
        (BlendMode::SoftLight, 0.704),
    ] {
        let ir = float_node(Blend::new(mode, "u", 0.2).opacity(1.0));
        let value = float_at(&ir, [0.8, 0.0]);
        assert!(close(value, expected), "{mode:?} answered {value}");
    }

    // The dark half of an overlay is the multiply half, doubled.
    let ir = float_node(Blend::new(BlendMode::Overlay, "u", 0.2).opacity(1.0));
    assert!(close(float_at(&ir, [0.25, 0.0]), 0.1));

    // A dissolve reads its opacity as a threshold rather than as a weight, so
    // every texel is one side or the other and never a blend of them.
    let dissolve =
        |opacity: f32| float_node(Blend::new(BlendMode::Dissolve, 0.25, 0.75).opacity(opacity));
    let none = dissolve(0.0);
    let all = dissolve(1.0);
    let half = dissolve(0.5);
    let mut taken = 0_usize;
    let mut left = 0_usize;
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 7) / 8.0];
        assert!(
            close(float_at(&none, uv), 0.25),
            "no opacity is the backdrop"
        );
        assert!(close(float_at(&all, uv), 0.75), "full opacity is the blend");
        let value = float_at(&half, uv);
        if close(value, 0.75) {
            taken += 1;
        } else {
            assert!(close(value, 0.25), "a dissolve never mixes: {value}");
            left += 1;
        }
    }
    assert!(taken > 8 && left > 8, "{taken} taken and {left} left");
}

#[test]
fn a_gradient_is_linear_between_its_stops_and_flat_outside_them() {
    let ir = wired(|builder| {
        builder
            .node(
                "n",
                Colorize::new("u").gradient([
                    (0.0, [0.0, 0.0, 0.0]),
                    (0.5, [1.0, 0.0, 0.0]),
                    (1.0, [1.0, 1.0, 1.0]),
                ]),
            )
            .output(PbrOutput::new().base_color("n"))
    });
    for (u, expected) in [
        (0.0, [0.0, 0.0, 0.0]),
        (0.25, [0.5, 0.0, 0.0]),
        (0.5, [1.0, 0.0, 0.0]),
        (0.75, [1.0, 0.5, 0.5]),
        (1.0, [1.0, 1.0, 1.0]),
    ] {
        let colour = colour_at(&ir, [u, 0.0]);
        assert!(close3(colour, expected), "at {u}: {colour:?}");
    }

    // Past the ends the first and last stop hold, rather than the ramp
    // running on into colours no stop names.
    let ir = wired(|builder| {
        builder
            .node(
                "n",
                Colorize::new("u").gradient([(0.25, [0.2, 0.4, 0.6]), (0.75, [0.8, 0.6, 0.4])]),
            )
            .output(PbrOutput::new().base_color("n"))
    });
    assert!(close3(colour_at(&ir, [0.0, 0.0]), [0.2, 0.4, 0.6]));
    assert!(close3(colour_at(&ir, [1.0, 0.0]), [0.8, 0.6, 0.4]));
    assert!(close3(colour_at(&ir, [0.5, 0.0]), [0.5, 0.5, 0.5]));
}

#[test]
fn every_math_operator_computes_what_it_says_it_does() {
    // Binary, over a = u = 0.75 and b = v = 0.25.
    for (op, expected) in [
        (MathOp::Add, 1.0),
        (MathOp::Sub, 0.5),
        (MathOp::Mul, 0.187_5),
        (MathOp::Div, 3.0),
        (MathOp::Min, 0.25),
        (MathOp::Max, 0.75),
        (MathOp::Pow, 0.930_604_9),
        // One where `a` is at least `b`.
        (MathOp::Step, 1.0),
        // The ramp runs one unit from `b`, so 0.75 is halfway up it.
        (MathOp::Smoothstep, 0.5),
        // The angle of (b, a) in turns: an eighth of a turn would be 0.125,
        // and this one is steeper than that.
        (MathOp::Atan2, 0.198_791_8),
    ] {
        let ir = float_node(Math::new(op, "u", "v"));
        let value = float_at(&ir, [0.75, 0.25]);
        assert!(close(value, expected), "{op:?} answered {value}");
    }

    // The midpoint of that ramp is where a smoothstep and a plain mix agree,
    // so the curve is read a quarter of the way up instead, where `t * t *
    // (3 - 2t)` is 0.156_25 and a linear ramp would answer 0.25.
    let ir = float_node(Math::new(MathOp::Smoothstep, "u", "v"));
    assert!(close(float_at(&ir, [0.5, 0.25]), 0.156_25));

    // Unary, over a = u = 0.75. The angles are turns, so three quarters of
    // one is a full sine wave less a quarter.
    for (op, expected) in [
        (MathOp::Abs, 0.75),
        (MathOp::Sqrt, 0.866_025_4),
        (MathOp::Floor, 0.0),
        (MathOp::Fract, 0.75),
        (MathOp::Sin, -1.0),
        (MathOp::Cos, 0.0),
        (MathOp::Log2, -0.415_037_5),
        (MathOp::Exp2, 1.681_792_8),
    ] {
        let ir = float_node(Math::unary(op, "u"));
        let value = float_at(&ir, [0.75, 0.25]);
        assert!(close(value, expected), "{op:?} answered {value}");
    }

    // A square root below zero is zero rather than the NaN that would spread
    // through every filter after it, and a division by zero is zero for the
    // same reason.
    let ir = float_node(Math::new(MathOp::Sqrt, "u", 0.0));
    assert!(close(float_at(&ir, [-4.0, 0.0]), 0.0));
    let ir = float_node(Math::new(MathOp::Div, "u", 0.0));
    assert!(close(float_at(&ir, [0.5, 0.0]), 0.0));

    // A unary operator ignores `b`, and ignores it in the answer's width as
    // well: the node is `a`'s float however wide `b` was, and the three equal
    // lanes below are the broadcast the base colour port asked for rather than
    // a join the operator never made.
    let material = MaterialGraph::builder("test:unary")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("n", Math::new(MathOp::Abs, "u", [0.1, 0.2, 0.3]))
        .output(PbrOutput::new().base_color("n"))
        .build()
        .unwrap();
    assert_eq!(
        material.port("n").unwrap().value_type,
        ValueType::Float,
        "a unary operator answers in `a` alone"
    );
    let ir = lower(&material, Target::Bake).unwrap();
    let colour = colour_at(&ir, [0.4, 0.0]);
    assert!(close3(colour, [0.4, 0.4, 0.4]), "{colour:?}");
}

#[test]
fn invert_and_mix_are_the_two_one_liners() {
    let ir = float_node(Invert::new("u"));
    assert!(close(float_at(&ir, [0.3, 0.0]), 0.7));

    let ir = float_node(Mix::new("u", 1.0, 0.25));
    assert!(close(float_at(&ir, [0.2, 0.0]), 0.4));

    // A colour on one side widens both: the join is what the port promised.
    let ir = wired(|builder| {
        builder
            .node("n", Mix::new([1.0, 0.0, 0.0], "u", 0.5))
            .output(PbrOutput::new().base_color("n"))
    });
    let colour = colour_at(&ir, [0.4, 0.0]);
    assert!(close3(colour, [0.7, 0.2, 0.2]), "{colour:?}");
}

/// The showcase's integer hash, from the hand-written pixel loop that came
/// before the graphs.
///
/// This and [`showcase_noise`] are an independent oracle: what has to hold is
/// that the graph's `Noise` is this function and not merely one like it, so
/// they are written out here rather than imported from the crate under test.
fn showcase_hash(x: u32, y: u32) -> f64 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    f64::from(h ^ (h >> 16)) / f64::from(u32::MAX)
}

/// The showcase's value noise, copied from the same file.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a verbatim copy of the oracle, whose own casts are bounded by \
              the texture dimensions and the octave frequencies"
)]
fn showcase_noise(u: f64, v: f64, frequency: u32) -> f64 {
    let x = u * f64::from(frequency);
    let y = v * f64::from(frequency);
    let ix = x.floor() as u32;
    let iy = y.floor() as u32;
    let smooth = |v: f64| v * v * (3.0 - 2.0 * v);
    let sx = smooth(x.fract());
    let sy = smooth(y.fract());
    let lower_left = showcase_hash(ix % frequency, iy % frequency);
    let lower_right = showcase_hash((ix + 1) % frequency, iy % frequency);
    let upper_left = showcase_hash(ix % frequency, (iy + 1) % frequency);
    let upper_right = showcase_hash((ix + 1) % frequency, (iy + 1) % frequency);
    (lower_left + (lower_right - lower_left) * sx) * (1.0 - sy)
        + (upper_left + (upper_right - upper_left) * sx) * sy
}

#[test]
fn the_value_noise_is_the_one_the_study_was_written_with() {
    // The four frequencies the study's pixel loop uses. The coordinates are
    // dyadic and the frequencies are powers of two, so `u * frequency` is
    // exact in both `f64` and `f32` and the two lattices agree on which cell
    // a texel falls in: what is compared is the arithmetic rather than a
    // rounding of the input to it. The offsets land the texel inside a cell,
    // where all four corners and both weights are carrying something.
    for frequency in [4_u32, 8, 32, 128] {
        let ir = float_node(Noise::value().period(frequency));
        let mut compared = 0_usize;
        for i in 0_u16..32 {
            for j in 0_u16..32 {
                let u = f32::from(i) / 32.0 + 1.0 / 512.0;
                let v = f32::from(j) / 32.0 + 3.0 / 512.0;
                let expected = showcase_noise(f64::from(u), f64::from(v), frequency);
                let value = f64::from(float_at(&ir, [u, v]));
                assert!(
                    (value - expected).abs() < 1e-5,
                    "noise({u}, {v}, {frequency}): {value} not {expected}"
                );
                compared += 1;
            }
        }
        assert_eq!(compared, 1024, "the grid is what it was meant to be");
    }
}

#[test]
fn a_perlin_noise_is_a_half_at_every_lattice_corner() {
    // Perlin's value is the interpolated projection of the offset from each
    // corner onto that corner's gradient, and on a corner every offset is
    // zero: the noise is exactly zero there whatever the gradients were. The
    // node carries it in `0..=1`, so exactly zero reads as exactly a half.
    let ir = float_node(Noise::perlin().period(4).seed(2));
    for uv in [[0.0_f32, 0.0_f32], [0.25, 0.5], [0.75, 0.75], [0.5, 0.25]] {
        let value = float_at(&ir, uv);
        assert!(close(value, 0.5), "at {uv:?}: {value}");
    }
    // The lattice wraps at the period, so the corner past the last is the
    // first: this is the same corner read from the other side of the seam.
    assert!(close(float_at(&ir, [1.0, 0.0]), 0.5));

    // Between corners it is not a half, and it stays inside the unit interval
    // the node promises.
    assert!(!close(float_at(&ir, [0.125, 0.125]), 0.5));
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 13) / 16.0];
        let value = float_at(&ir, uv);
        assert!((0.0..=1.0).contains(&value), "at {uv:?}: {value}");
    }

    // A seed moves the field without moving the lattice, and the corners stay
    // corners.
    let other = float_node(Noise::perlin().period(4).seed(9));
    assert!(close(float_at(&other, [0.25, 0.5]), 0.5));
    assert!(!close(
        float_at(&ir, [0.125, 0.375]),
        float_at(&other, [0.125, 0.375])
    ));
}

/// Where a [`Voronoi`] puts the feature point of one cell, in UV.
///
/// The lowering strays the point from its cell's centre by half the jitter
/// times a hash read in `-1..=1`, which at a jitter of one is the cell's own
/// hash across the whole cell. Computed here from the hash rather than read off
/// the node, so that the test is an oracle rather than a copy.
fn feature_point(cell: [u32; 2], period: u32, seed: u32, jitter: f32) -> [f32; 2] {
    const SALT: u32 = 0x9E37_79B9;
    let stray = |hash: f32| 0.5 + jitter * 0.5 * hash.mul_add(2.0, -1.0);
    let x = f32::from(u16::try_from(cell[0]).unwrap()) + stray(hash2(cell[0], cell[1], seed));
    let y = f32::from(u16::try_from(cell[1]).unwrap())
        + stray(hash2(cell[0], cell[1], seed.wrapping_add(SALT)));
    #[expect(clippy::cast_precision_loss, reason = "a period is a small count")]
    let scale = period as f32;
    [x / scale, y / scale]
}

#[test]
fn a_voronoi_distance_is_zero_at_a_feature_point() {
    // With no jitter every point sits at the centre of its own cell, so the
    // distance there is zero and the furthest a texel gets is the corner where
    // four cells meet, half a cell away in each axis.
    let ir = float_node(Voronoi::new().period(4).jitter(0.0));
    assert!(close(float_at(&ir, [0.125, 0.125]), 0.0), "a cell centre");
    assert!(close(float_at(&ir, [0.625, 0.375]), 0.0), "and another");
    let corner = float_at(&ir, [0.25, 0.25]);
    assert!(close(corner, 0.5_f32.hypot(0.5)), "{corner}");

    // With jitter the point is wherever the cell's own hash put it, and the
    // distance is zero there too.
    let seeded = Voronoi::new().period(4).seed(5).jitter(1.0);
    let ir = float_node(seeded);
    for cell in [[0_u32, 0_u32], [1, 2], [3, 3]] {
        let point = feature_point(cell, 4, 5, 1.0);
        let value = float_at(&ir, point);
        assert!(value.abs() < 1e-5, "cell {cell:?} at {point:?}: {value}");
    }

    // The other three outputs, read at the same points. A cell id is one
    // number over a whole cell; an edge distance is *not* zero at a feature
    // point, because a point is as far from its own boundary as a cell gets;
    // and the border is the band along that boundary, so it is zero there.
    let ids = float_node(seeded.output(VoronoiOutput::Cell));
    let edges = float_node(seeded.output(VoronoiOutput::Edge));
    let border = float_node(seeded.output(VoronoiOutput::Border).width(0.05));
    let point = feature_point([1, 2], 4, 5, 1.0);
    let id = float_at(&ids, point);
    assert!((0.0..=1.0).contains(&id));
    assert!(
        close(id, float_at(&ids, [point[0] + 0.01, point[1]])),
        "a cell id is one number per cell"
    );
    assert!(!close(id, float_at(&ids, [point[0] + 0.25, point[1]])));
    assert!(float_at(&edges, point) > 0.05, "a point is not a boundary");
    assert!(close(float_at(&border, point), 0.0));

    // And the metric is the one the node names: with no jitter, a texel a
    // quarter of a cell along each axis from a centre is a quarter plus a
    // quarter to Manhattan and a quarter to Chebyshev.
    let plain = Voronoi::new().period(4).jitter(0.0);
    let manhattan = float_node(plain.metric(VoronoiMetric::Manhattan));
    let chebyshev = float_node(plain.metric(VoronoiMetric::Chebyshev));
    let offset = [0.125 + 0.25 / 4.0, 0.125 + 0.25 / 4.0];
    assert!(close(float_at(&manhattan, offset), 0.5));
    assert!(close(float_at(&chebyshev, offset), 0.25));

    // And the offset is the vector from the texel to that same point, in UV
    // rather than in the cell units the distances above are in: the search
    // computes it on its way to the distance, and this is the output that
    // stops throwing it away. With no jitter it is the vector to the centre.
    let centred = vec2_at(plain.output(VoronoiOutput::Offset), [0.0625, 0.0625]);
    assert!(
        close(centred[0], 0.0625) && close(centred[1], 0.0625),
        "{centred:?}"
    );
    // With jitter it points at wherever the cell's own hash put the point, and
    // its length is the distance the node already answered, over the period.
    let point = feature_point([1, 2], 4, 5, 1.0);
    let texel = [point[0] + 0.03, point[1] - 0.02];
    let towards = vec2_at(seeded.output(VoronoiOutput::Offset), texel);
    assert!(
        close(towards[0], point[0] - texel[0]) && close(towards[1], point[1] - texel[1]),
        "{towards:?} does not point at {point:?} from {texel:?}"
    );
    let reach = towards[0].hypot(towards[1]);
    assert!(
        close(reach, float_at(&float_node(seeded), texel) / 4.0),
        "the offset's length is the distance in UV: {reach}"
    );
}

#[test]
fn a_wall_carries_a_bevel_an_id_and_a_fill_beside_its_mask() {
    let wall = |output| {
        float_node(
            Bricks::new()
                .rows(4)
                .columns(2)
                .offset(0.5)
                .mortar(0.1)
                .bevel(0.2)
                .output(output),
        )
    };
    let mask = wall(BrickOutput::Mask);
    let bevel = wall(BrickOutput::Bevel);
    let id = wall(BrickOutput::Id);
    let fill = wall(BrickOutput::Fill);

    // The middle of a brick is as far into the face as a texel gets, so the
    // bevel is at its top there and the mask is one.
    let middle = [0.25, 0.125];
    assert!(close(float_at(&mask, middle), 1.0));
    assert!(close(float_at(&bevel, middle), 1.0));
    // In the mortar both are zero.
    let joint = [0.0, 0.125];
    assert!(close(float_at(&mask, joint), 0.0));
    assert!(close(float_at(&bevel, joint), 0.0));
    // And on the way down the bevel is the ramp the mask is a step at: the
    // brick-local coordinate here is a tenth past the mortar, which is halfway
    // up a bevel of a fifth.
    // Brick-local 0.2, which is the mortar plus half the bevel.
    let shoulder = [0.1, 0.125];
    assert!(close(float_at(&mask, shoulder), 1.0));
    assert!(close(float_at(&bevel, shoulder), 0.5));

    // A bevel of nothing is the mask itself, edge for edge.
    let hard = float_node(Bricks::new().rows(4).columns(2).mortar(0.1).bevel(0.0));
    let plain = float_node(Bricks::new().rows(4).columns(2).mortar(0.1));
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 9) / 16.0];
        let bevelled = float_at(
            &float_node(
                Bricks::new()
                    .rows(4)
                    .columns(2)
                    .mortar(0.1)
                    .bevel(0.0)
                    .output(BrickOutput::Bevel),
            ),
            uv,
        );
        assert!(close(bevelled, float_at(&plain, uv)), "at {uv:?}");
    }
    assert!(close(float_at(&hard, [0.25, 0.125]), 1.0));

    // An id is one number per brick: the same anywhere inside one, different
    // in the brick next door, and the same brick again a whole repeat along.
    let here = float_at(&id, [0.15, 0.1]);
    assert!(close(here, float_at(&id, [0.2, 0.2])), "one brick");
    assert!(!close(here, float_at(&id, [0.65, 0.1])), "the next one");
    assert!(close(here, float_at(&id, [1.15, 0.1])), "and the wrap");
    assert!((0.0..=1.0).contains(&here));

    // A fill runs from zero to one across a brick, whatever the bond did to
    // where that brick starts.
    assert!(close(float_at(&fill, [0.0, 0.125]), 0.0));
    assert!(close(float_at(&fill, [0.25, 0.125]), 0.5));
    // Row one is laid half a brick along, so its fill is shifted with it.
    assert!(close(float_at(&fill, [0.25, 0.375]), 0.0));
}

#[test]
fn a_grid_of_tiles_is_the_face_between_its_gaps() {
    let tiles = |output| {
        float_node(
            Tiles::new()
                .pattern(TilePattern::Grid)
                .columns(4)
                .rows(4)
                .gap(0.1)
                .bevel(0.05)
                .output(output),
        )
    };
    let mask = tiles(BrickOutput::Mask);
    let bevel = tiles(BrickOutput::Bevel);
    let id = tiles(BrickOutput::Id);
    let fill = tiles(BrickOutput::Fill);

    // A cell is a quarter of the repeat, so the middle of one is an eighth
    // from its own boundary and the gap takes half of a tenth off each side.
    assert!(close(float_at(&mask, [0.125, 0.125]), 1.0));
    assert!(close(float_at(&bevel, [0.125, 0.125]), 1.0));
    // The boundary itself is gap, and so is everything within half a gap of it.
    assert!(close(float_at(&mask, [0.25, 0.125]), 0.0));
    assert!(close(float_at(&mask, [0.21, 0.125]), 0.0));
    // A texel further in than half the gap is face, and the bevel ramps over
    // the twentieth past that: a sixteenth of the repeat is a quarter of the
    // way across a cell, which is a sixteenth of the repeat from the boundary.
    assert!(close(float_at(&mask, [0.187_5, 0.125]), 1.0));
    assert!(close(float_at(&bevel, [0.25 - 0.075, 0.125]), 0.5));

    // One id per cell, and the fill runs across one.
    let here = float_at(&id, [0.1, 0.1]);
    assert!(close(here, float_at(&id, [0.2, 0.2])));
    assert!(!close(here, float_at(&id, [0.3, 0.1])));
    assert!(close(float_at(&fill, [0.125, 0.125]), 0.5));
    assert!(close(float_at(&fill, [0.0625, 0.125]), 0.25));
}

#[test]
fn a_hexagonal_lattice_offsets_its_odd_rows_by_half_a_cell() {
    let hex = |output| {
        float_node(
            Tiles::new()
                .pattern(TilePattern::Hex)
                .columns(4)
                .rows(4)
                .gap(0.02)
                .output(output),
        )
    };
    let mask = hex(BrickOutput::Mask);
    let id = hex(BrickOutput::Id);

    // Row zero has its centres at half-cells; row one has them a further half
    // a cell along, which is what makes the lattice hexagonal.
    for centre in [[0.125_f32, 0.125_f32], [0.375, 0.125], [0.25, 0.375]] {
        assert!(close(float_at(&mask, centre), 1.0), "at {centre:?}");
    }
    // Halfway between two centres of one row is a boundary, and the gap opens
    // on both sides of it.
    assert!(close(float_at(&mask, [0.25, 0.125]), 0.0));

    // The cell a texel belongs to is the nearest centre's, so a texel just
    // either side of that boundary belongs to different cells.
    let left = float_at(&id, [0.24, 0.125]);
    let right = float_at(&id, [0.26, 0.125]);
    assert!(!close(left, right), "{left} and {right}");
    assert!(close(left, float_at(&id, [0.125, 0.125])), "the left cell");

    // A row above is offset, so the cell over a row-zero centre is not that
    // centre's own: this is the claim the even row count rests on.
    assert!(!close(
        float_at(&id, [0.125, 0.125]),
        float_at(&id, [0.125, 0.375])
    ));
}

#[test]
fn a_herringbone_weave_lays_two_cell_tiles_at_right_angles() {
    // The rule is `k = (column + row) mod 4`: horizontal where `k` is zero or
    // one, vertical where it is two or three, and the tile's origin is that
    // many cells back. So cells (0,0) and (1,0) are one horizontal tile, and
    // cells (2,0) and (2,1) are one vertical tile.
    let id = float_node(
        Tiles::new()
            .pattern(TilePattern::Herringbone)
            .columns(4)
            .rows(4)
            .gap(0.01)
            .output(BrickOutput::Id),
    );
    let at_cell = |x: f32, y: f32| float_at(&id, [(x + 0.5) / 4.0, (y + 0.5) / 4.0]);
    let flat = at_cell(0.0, 0.0);
    assert!(close(flat, at_cell(1.0, 0.0)), "two cells of one tile");
    let upright = at_cell(2.0, 0.0);
    assert!(
        close(upright, at_cell(2.0, 1.0)),
        "and of the one beside it"
    );
    assert!(!close(flat, upright), "which is a different tile");

    // And the weave marches: the tile at (2,2) is the one at (0,0) moved two
    // cells along each axis, so it is a different tile with the same shape.
    assert!(!close(flat, at_cell(2.0, 2.0)));
    assert!(
        close(at_cell(2.0, 2.0), at_cell(3.0, 2.0)),
        "horizontal again"
    );

    // Every texel belongs to a tile, so the mask is one everywhere except in
    // the gaps, which is what "partitions the plane" comes to.
    let mask = float_node(
        Tiles::new()
            .pattern(TilePattern::Herringbone)
            .columns(4)
            .rows(4)
            .gap(0.0)
            .output(BrickOutput::Mask),
    );
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 11) / 16.0];
        assert!(close(float_at(&mask, uv), 1.0), "at {uv:?}");
    }
}

#[test]
fn a_pattern_is_the_wave_its_kind_names_along_each_axis() {
    // Stripes: a hard edge halfway through the repeat, in each axis, and the
    // product of the two.
    let stripes = float_node(Pattern::new(PatternKind::Stripes).x(2).y(1));
    assert!(
        close(float_at(&stripes, [0.1, 0.75]), 0.0),
        "first half in u"
    );
    assert!(close(float_at(&stripes, [0.3, 0.75]), 1.0), "second half");
    assert!(
        close(float_at(&stripes, [0.3, 0.25]), 0.0),
        "first half in v"
    );

    // Sine: a quarter of the way through the repeat is the crest.
    let sine = float_node(Pattern::new(PatternKind::Sine).mix(PatternMix::Max));
    assert!(close(float_at(&sine, [0.25, 0.25]), 1.0));
    assert!(close(float_at(&sine, [0.75, 0.75]), 0.0));
    assert!(close(float_at(&sine, [0.0, 0.0]), 0.5));

    // Triangle: up over the first half and down over the second.
    let triangle = float_node(Pattern::new(PatternKind::Triangle).mix(PatternMix::Min));
    assert!(close(float_at(&triangle, [0.5, 0.5]), 1.0));
    assert!(close(float_at(&triangle, [0.25, 0.25]), 0.5));
    assert!(close(float_at(&triangle, [0.0, 0.0]), 0.0));

    // A checker is the difference of the two square waves, which is the
    // exclusive-or of them.
    let checker = float_node(Pattern::new(PatternKind::Checker).x(2).y(2));
    assert!(close(float_at(&checker, [0.1, 0.1]), 0.0));
    assert!(close(float_at(&checker, [0.4, 0.1]), 1.0));
    assert!(close(float_at(&checker, [0.1, 0.4]), 1.0));
    assert!(close(float_at(&checker, [0.4, 0.4]), 0.0));

    // And a checker combined any other way is refused rather than answered as
    // a grid of bars, which is what the Stripes kind is for.
    let error = MaterialGraph::builder("test:checker")
        .node(
            "n",
            Pattern::new(PatternKind::Checker).mix(PatternMix::Multiply),
        )
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].mix");

    // The rest of the combinations, over waves of one and zero.
    for (mix, expected) in [
        (PatternMix::Multiply, 0.0),
        (PatternMix::Add, 1.0),
        (PatternMix::Max, 1.0),
        (PatternMix::Min, 0.0),
        (PatternMix::Average, 0.5),
        (PatternMix::Difference, 1.0),
    ] {
        let ir = float_node(Pattern::new(PatternKind::Stripes).x(1).y(1).mix(mix));
        let value = float_at(&ir, [0.75, 0.25]);
        assert!(close(value, expected), "{mix:?} answered {value}");
    }
}

#[test]
fn a_shape_is_one_inside_its_own_edge_and_zero_outside_it() {
    // A circle of radius 0.3 with a hard edge, read along one axis from the
    // centre of the repeat.
    let circle = float_node(Shape::new(ShapeKind::Circle).size(0.3).edge(0.0));
    assert!(close(float_at(&circle, [0.5, 0.5]), 1.0));
    assert!(close(float_at(&circle, [0.7, 0.5]), 1.0));
    assert!(close(float_at(&circle, [0.81, 0.5]), 0.0));
    // The corner of the repeat is further than any axis, so it is outside.
    assert!(close(float_at(&circle, [0.0, 0.0]), 0.0));

    // A soft edge takes the falloff *inward*, so the shape still fits the
    // repeat: one at size less the edge, zero at size.
    let soft = float_node(Shape::new(ShapeKind::Circle).size(0.3).edge(0.1));
    assert!(close(float_at(&soft, [0.5 + 0.2, 0.5]), 1.0));
    assert!(close(float_at(&soft, [0.5 + 0.25, 0.5]), 0.5));
    assert!(close(float_at(&soft, [0.5 + 0.3, 0.5]), 0.0));

    // A box is the Chebyshev distance: its corner is further out than its
    // sides, which is exactly what a circle's is not.
    let square = float_node(Shape::new(ShapeKind::Box).size(0.3).edge(0.0));
    assert!(close(float_at(&square, [0.5 + 0.29, 0.5 + 0.29]), 1.0));
    assert!(close(float_at(&square, [0.5 + 0.31, 0.5]), 0.0));

    // A polygon's `size` is the radius it reaches, so a vertex sits there and
    // the middle of a side is closer in by the apothem: a square polygon's
    // side passes through `size * cos(45)`.
    let polygon = float_node(Shape::new(ShapeKind::Polygon).sides(4).size(0.4).edge(0.0));
    assert!(
        close(float_at(&polygon, [0.5 + 0.39, 0.5]), 1.0),
        "a vertex"
    );
    assert!(close(float_at(&polygon, [0.5 + 0.41, 0.5]), 0.0));
    let apothem = 0.4 * std::f32::consts::FRAC_1_SQRT_2;
    let diagonal = apothem * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        close(
            float_at(&polygon, [0.5 + diagonal * 0.95, 0.5 + diagonal * 0.95]),
            1.0
        ),
        "inside the side"
    );
    assert!(close(
        float_at(&polygon, [0.5 + diagonal * 1.05, 0.5 + diagonal * 1.05]),
        0.0
    ));

    // A star reaches `size` at a point and half of it in the valley between
    // two of them.
    let star = float_node(Shape::new(ShapeKind::Star).sides(5).size(0.4).edge(0.0));
    assert!(close(float_at(&star, [0.5 + 0.38, 0.5]), 1.0), "a point");
    let valley = std::f32::consts::TAU / 10.0;
    let (sine, cosine) = valley.sin_cos();
    assert!(close(
        float_at(&star, [0.5 + 0.19 * cosine, 0.5 + 0.19 * sine]),
        1.0
    ));
    assert!(close(
        float_at(&star, [0.5 + 0.21 * cosine, 0.5 + 0.21 * sine]),
        0.0
    ));

    // A shape that would run off the repeat is refused, because the seam would
    // cut it and the period it claims would be a lie.
    let error = MaterialGraph::builder("test:big")
        .node("n", Shape::new(ShapeKind::Circle).size(0.48).edge(0.05))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].size");
}

#[test]
fn a_shape_that_was_already_writable_lowers_to_the_instruction_list_it_always_did() {
    // Recorded from the lowering as it stood before `Shape` grew a capsule, a
    // gear, a corner radius, a shell and a distance output. `ir_hash` hashes
    // the instruction stream itself — every op, every operand and every
    // constant's bits — so an equal hash is the claim that a shape an author
    // could already write is the same *expression* and not merely the same
    // picture. It has to be: the shipped textures were baked from it, and the
    // showcase goldens would otherwise be a hash to update rather than a bug
    // to fix.
    for (shape, length, hash) in [
        (Shape::default(), 14, 0x1959_d522_accd_1ad8_u64),
        (
            Shape::new(ShapeKind::Circle).size(0.3).edge(0.0),
            14,
            0x1e2c_a2b5_9eac_6e67,
        ),
        (
            Shape::new(ShapeKind::Box).size(0.3).edge(0.0),
            17,
            0x1a19_4187_a61b_4647,
        ),
        (
            Shape::new(ShapeKind::Box).size(0.4).edge(0.05),
            17,
            0x0056_0c69_34b1_d04c,
        ),
        (
            Shape::new(ShapeKind::Polygon).sides(4).size(0.4).edge(0.0),
            32,
            0x9966_b591_5846_9da6,
        ),
        (
            Shape::new(ShapeKind::Polygon)
                .sides(6)
                .size(0.35)
                .edge(0.05),
            32,
            0x78a7_70b3_8206_006b,
        ),
        (
            Shape::new(ShapeKind::Star).sides(5).size(0.4).edge(0.0),
            33,
            0x12e0_8aae_50ee_18ee,
        ),
        (
            Shape::new(ShapeKind::Star).sides(7).size(0.3).edge(0.08),
            33,
            0xb837_d255_2bd1_30f7,
        ),
    ] {
        let ir = float_node(shape);
        assert_eq!(ir.len(), length, "{shape:?}");
        assert_eq!(ir_hash(&ir), hash, "{shape:?}");
    }

    // And the same claim read off the picture rather than off the hash: the
    // default shape's mask at the centre of every texel of an eight-squared
    // grid, recorded the same way. A hash says the expression did not move; a
    // grid says what it answers, and the falloff values are where a drifting
    // ramp would show first.
    let soft = 0.798_094_9;
    let rows = [
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, soft, 1.0, 1.0, soft, 0.0, 0.0],
        [0.0, soft, 1.0, 1.0, 1.0, 1.0, soft, 0.0],
        [0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0],
        [0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0],
        [0.0, soft, 1.0, 1.0, 1.0, 1.0, soft, 0.0],
        [0.0, 0.0, soft, 1.0, 1.0, soft, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ];
    let ir = float_node(Shape::default());
    for (j, row) in rows.iter().enumerate() {
        for (i, expected) in row.iter().enumerate() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "two grid indices in 0..8, which f32 holds exactly"
            )]
            let uv = [(i as f32 + 0.5) / 8.0, (j as f32 + 0.5) / 8.0];
            let value = float_at(&ir, uv);
            assert!(close(value, *expected), "texel {i},{j} answered {value}");
        }
    }
}

#[test]
fn a_hollow_circle_is_one_only_between_its_two_radii() {
    // A ring of outer radius 0.4 with a shell a tenth thick, so the hole
    // reaches 0.3 and what is left is the band between the two radii.
    let ring = float_node(
        Shape::new(ShapeKind::Circle)
            .size(0.4)
            .edge(0.0)
            .hollow(0.1),
    );
    assert!(
        close(float_at(&ring, [0.5, 0.5]), 0.0),
        "the middle of the hole"
    );
    assert!(
        close(float_at(&ring, [0.5 + 0.29, 0.5]), 0.0),
        "just inside the hole"
    );
    assert!(close(float_at(&ring, [0.5 + 0.31, 0.5]), 1.0), "the band");
    assert!(close(float_at(&ring, [0.5 + 0.39, 0.5]), 1.0), "the rim");
    assert!(
        close(float_at(&ring, [0.5 + 0.41, 0.5]), 0.0),
        "past the rim"
    );
    // And the same band in every direction, which is what says the shell is
    // taken off the distance rather than off one axis of it.
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    assert!(close(
        float_at(&ring, [0.5 + 0.35 * diagonal, 0.5 + 0.35 * diagonal]),
        1.0
    ));
    assert!(close(
        float_at(&ring, [0.5 + 0.2 * diagonal, 0.5 + 0.2 * diagonal]),
        0.0
    ));

    // A shell thicker than the shape closes the hole again: there is no
    // distance far enough inside to fall past the far side of the band.
    let solid = float_node(
        Shape::new(ShapeKind::Circle)
            .size(0.4)
            .edge(0.0)
            .hollow(0.5),
    );
    assert!(close(float_at(&solid, [0.5, 0.5]), 1.0));

    // The same field over a box is a frame, corners included.
    let frame = float_node(Shape::new(ShapeKind::Box).size(0.4).edge(0.0).hollow(0.1));
    assert!(close(float_at(&frame, [0.5 + 0.35, 0.5]), 1.0), "a rail");
    assert!(close(float_at(&frame, [0.5 + 0.2, 0.5]), 0.0), "the middle");
    assert!(
        close(float_at(&frame, [0.5 + 0.35, 0.5 + 0.35]), 1.0),
        "a corner of the frame"
    );
}

#[test]
fn a_capsule_is_a_segment_with_a_cap_at_each_end() {
    // A segment two fifths long lying along u, with caps of radius 0.08: it
    // reaches 0.28 along u and 0.08 across it.
    let capsule = float_node(
        Shape::new(ShapeKind::Capsule)
            .size(0.08)
            .edge(0.0)
            .length(0.4),
    );
    assert!(close(float_at(&capsule, [0.5, 0.5]), 1.0), "the middle");
    assert!(
        close(float_at(&capsule, [0.5 + 0.19, 0.5]), 1.0),
        "the segment"
    );
    assert!(close(float_at(&capsule, [0.5 + 0.27, 0.5]), 1.0), "the cap");
    assert!(close(float_at(&capsule, [0.5 + 0.29, 0.5]), 0.0), "past it");
    assert!(
        close(float_at(&capsule, [0.5, 0.5 + 0.07]), 1.0),
        "across it"
    );
    assert!(close(float_at(&capsule, [0.5, 0.5 + 0.09]), 0.0));
    // The cap is round rather than square: a point out at forty-five degrees
    // from the cap's own centre is inside at the radius and outside past it.
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let inside = 0.075 * diagonal;
    let outside = 0.085 * diagonal;
    assert!(close(
        float_at(&capsule, [0.5 + 0.2 + inside, 0.5 + inside]),
        1.0
    ));
    assert!(close(
        float_at(&capsule, [0.5 + 0.2 + outside, 0.5 + outside]),
        0.0
    ));
    // A capsule of no length is the disc its caps are.
    let disc = float_node(
        Shape::new(ShapeKind::Capsule)
            .size(0.3)
            .edge(0.0)
            .length(0.0),
    );
    assert!(close(float_at(&disc, [0.5 + 0.29, 0.5]), 1.0));
    assert!(close(float_at(&disc, [0.5 + 0.31, 0.5]), 0.0));

    // The capsule is the one kind whose reach is not `size`, so the rule that
    // keeps a shape inside its repeat is read against half its length as well,
    // and a capsule that would be cut at the seam is refused there.
    let error = MaterialGraph::builder("test:long")
        .node(
            "n",
            Shape::new(ShapeKind::Capsule)
                .size(0.2)
                .edge(0.0)
                .length(0.7),
        )
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].length");
}

#[test]
fn a_gear_reaches_its_size_at_a_tooth_and_its_root_between_them() {
    // Eight teeth cut a tenth deep into a disc of 0.4, so a crest is at 0.4 and
    // the root circle between two of them stands at 0.3. A tooth points along
    // +u, where a polygon puts its vertex, so the two agree about which way a
    // shape faces.
    let gear = float_node(
        Shape::new(ShapeKind::Gear)
            .sides(8)
            .size(0.4)
            .edge(0.0)
            .depth(0.1),
    );
    let at_angle = |radius: f32, angle: f32| {
        let (sine, cosine) = angle.sin_cos();
        float_at(&gear, [0.5 + radius * cosine, 0.5 + radius * sine])
    };
    let pitch = std::f32::consts::TAU / 8.0;
    for tooth in 0..8 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a tooth index in 0..8, which f32 holds exactly"
        )]
        let centre = tooth as f32 * pitch;
        assert!(
            close(at_angle(0.39, centre), 1.0),
            "the crest of tooth {tooth}"
        );
        assert!(close(at_angle(0.41, centre), 0.0), "past that crest");
        // Half a pitch round is the root between this tooth and the next.
        let root = centre + pitch * 0.5;
        assert!(close(at_angle(0.29, root), 1.0), "inside root {tooth}");
        assert!(close(at_angle(0.31, root), 0.0), "the gap at root {tooth}");
    }
    // A gear of no depth is the disc it was cut from, at every angle.
    let disc = float_node(
        Shape::new(ShapeKind::Gear)
            .sides(8)
            .size(0.4)
            .edge(0.0)
            .depth(0.0),
    );
    for step in 0..16 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a step index in 0..16, which f32 holds exactly"
        )]
        let angle = step as f32 * pitch / 2.0;
        let (sine, cosine) = angle.sin_cos();
        assert!(close(
            float_at(&disc, [0.5 + 0.39 * cosine, 0.5 + 0.39 * sine]),
            1.0
        ));
        assert!(close(
            float_at(&disc, [0.5 + 0.41 * cosine, 0.5 + 0.41 * sine]),
            0.0
        ));
    }

    // The teeth are cut inward from `size`, so a depth past it would put the
    // root circle behind the centre and is refused at its own field.
    let error = MaterialGraph::builder("test:deep")
        .node("n", Shape::new(ShapeKind::Gear).size(0.3).depth(0.4))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].depth");
}

#[test]
fn rounding_takes_the_corners_off_a_box_and_leaves_a_disc_where_it_was() {
    let square = float_node(Shape::new(ShapeKind::Box).size(0.4).edge(0.0));
    let rounded = float_node(Shape::new(ShapeKind::Box).size(0.4).edge(0.0).round(0.15));
    // The middle of a side is where the two agree: the shape is drawn the
    // radius smaller and the distance is then moved back out by it, which
    // leaves a straight edge exactly where it was.
    assert!(close(float_at(&rounded, [0.5 + 0.39, 0.5]), 1.0), "a side");
    assert!(close(float_at(&rounded, [0.5 + 0.41, 0.5]), 0.0));
    // The corner is the whole of the difference.
    assert!(
        close(float_at(&square, [0.5 + 0.39, 0.5 + 0.39]), 1.0),
        "the square corner"
    );
    assert!(
        close(float_at(&rounded, [0.5 + 0.39, 0.5 + 0.39]), 0.0),
        "the corner taken off"
    );
    // What stands there instead is an arc of radius 0.15 about (0.25, 0.25).
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let inside = 0.25 + 0.145 * diagonal;
    let outside = 0.25 + 0.155 * diagonal;
    assert!(close(float_at(&rounded, [0.5 + inside, 0.5 + inside]), 1.0));
    assert!(close(
        float_at(&rounded, [0.5 + outside, 0.5 + outside]),
        0.0
    ));

    // A disc has no corner to take, so the radius folds away rather than
    // costing two instructions to answer the shape it started with.
    let circle = float_node(Shape::new(ShapeKind::Circle).size(0.3).edge(0.0));
    let rounded_circle = float_node(Shape::new(ShapeKind::Circle).size(0.3).edge(0.0).round(0.2));
    assert_eq!(ir_hash(&circle), ir_hash(&rounded_circle));

    // A radius wider than the shape it rounds would ask for a negative shape,
    // and is refused at its own field.
    let error = MaterialGraph::builder("test:round")
        .node("n", Shape::new(ShapeKind::Box).size(0.2).round(0.3))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].round");
}

#[test]
fn a_shapes_distance_output_is_the_field_its_mask_ramps() {
    let distance = float_node(
        Shape::new(ShapeKind::Circle)
            .size(0.3)
            .edge(0.1)
            .output(ShapeOutput::Distance),
    );
    // Signed, in UV, negative inside and unclamped either way: a texel a tenth
    // past the rim reads a tenth, and the centre reads the whole radius.
    assert!(close(float_at(&distance, [0.5, 0.5]), -0.3));
    assert!(close(float_at(&distance, [0.5 + 0.3, 0.5]), 0.0));
    assert!(close(float_at(&distance, [0.5 + 0.4, 0.5]), 0.1));
    // The edge does not touch it, which is what makes it the field a boolean
    // combines rather than a picture of one.
    let hard = float_node(
        Shape::new(ShapeKind::Circle)
            .size(0.3)
            .edge(0.0)
            .output(ShapeOutput::Distance),
    );
    assert!(close(float_at(&hard, [0.5 + 0.4, 0.5]), 0.1));

    // And the mask is that same distance through the inward ramp: one at the
    // edge's width inside the boundary, zero at it, smooth between.
    let mask = float_node(Shape::new(ShapeKind::Circle).size(0.3).edge(0.1));
    let ramp = |d: f32| {
        let t = ((d + 0.1) / 0.1).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    };
    for radius in [0.0_f32, 0.1, 0.19, 0.22, 0.25, 0.28, 0.3, 0.35] {
        let uv = [0.5 + radius, 0.5];
        let expected = ramp(float_at(&distance, uv));
        let value = float_at(&mask, uv);
        assert!(
            close(value, expected),
            "at {radius} the mask answered {value}"
        );
    }
}

#[test]
fn a_mask_over_a_shapes_distance_is_the_shapes_own_mask() {
    // The claim the ramp was factored out for. A shape read as a mask and the
    // same shape read as a distance and masked afterwards are not merely close:
    // they are one instruction list, because both go through `inward_ramp`, and
    // `ir_hash` hashes every op, every operand and every constant's bits. If
    // they ever stop being one list, a boolean between two shapes stops being
    // readable as a shape, which is the only reason the distance output exists.
    for shape in [
        Shape::default(),
        Shape::new(ShapeKind::Circle).size(0.3).edge(0.0),
        Shape::new(ShapeKind::Box).size(0.4).edge(0.02).round(0.1),
        Shape::new(ShapeKind::Star).sides(5).size(0.35).edge(0.08),
        Shape::new(ShapeKind::Gear)
            .sides(9)
            .size(0.4)
            .depth(0.1)
            .edge(0.03)
            .hollow(0.06),
    ] {
        let own = float_node(shape);
        let ramped = float_node(SdfMask::new(shape.output(ShapeOutput::Distance)).edge(shape.edge));
        assert_eq!(own.len(), ramped.len(), "{shape:?}");
        assert_eq!(ir_hash(&own), ir_hash(&ramped), "{shape:?}");
        // And read off the picture as well as off the hash, at the centre of
        // every texel of an eight-squared grid: the two agree bit for bit,
        // which is a stronger claim than the margin everything else here uses.
        for j in 0..8_u16 {
            for i in 0..8_u16 {
                let uv = [f32::from(i) / 8.0 + 0.0625, f32::from(j) / 8.0 + 0.0625];
                let (left, right) = (float_at(&own, uv), float_at(&ramped, uv));
                assert_eq!(left.to_bits(), right.to_bits(), "{shape:?} at {uv:?}");
            }
        }
    }

    // The edge is the node's own, so a mask may be ramped harder or softer than
    // the shape it came from without the shape being rewritten. A hard one is
    // one inside and zero out, with the boundary itself counting as inside.
    let hard = float_node(SdfMask::new(disc_distance(0.3)).edge(0.0));
    assert!(close(float_at(&hard, [0.5, 0.5]), 1.0));
    assert!(close(float_at(&hard, [0.5 + 0.29, 0.5]), 1.0));
    assert!(close(float_at(&hard, [0.5 + 0.31, 0.5]), 0.0));
    // A width past the unit is a width in UV that is wider than the repeat.
    let refused = MaterialGraph::builder("test:mask")
        .node("n", SdfMask::new(0.0).edge(1.5))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .expect_err("an edge past the unit");
    assert_eq!(refused.path, "nodes[n].edge");
}

/// A disc of the given radius, centred in the repeat, as a signed distance.
fn disc_distance(size: f32) -> Shape {
    Shape::new(ShapeKind::Circle)
        .size(size)
        .edge(0.0)
        .output(ShapeOutput::Distance)
}

#[test]
fn a_combine_with_no_smoothing_is_exactly_the_hard_boolean() {
    // Two signed fields that are not distances of anything in particular,
    // because a boolean is arithmetic over numbers and this is the part of it
    // that has no geometry in it: at a fillet of no width the node is `min`,
    // `max` and `max(a, -b)`, and the comparison is by bits.
    let signed = |seed: u32| Levels::new(Noise::value().period(4).seed(seed)).out_range(-0.5, 0.5);
    let against = |op: SdfOp, hard: Node| {
        let combined = wired(|builder| {
            builder
                .node("a", signed(1))
                .node("b", signed(2))
                .node("n", SdfCombine::new(op, "a", "b"))
                .output(PbrOutput::new().roughness("n"))
        });
        let written = wired(|builder| {
            builder
                .node("a", signed(1))
                .node("b", signed(2))
                .node("n", hard)
                .output(PbrOutput::new().roughness("n"))
        });
        assert_eq!(ir_hash(&combined), ir_hash(&written), "{op:?}");
        for j in 0..16_u16 {
            for i in 0..16_u16 {
                let uv = [f32::from(i) / 16.0 + 0.03125, f32::from(j) / 16.0 + 0.03125];
                let (left, right) = (float_at(&combined, uv), float_at(&written, uv));
                assert_eq!(left.to_bits(), right.to_bits(), "{op:?} at {uv:?}");
            }
        }
    };
    against(SdfOp::Union, Math::new(MathOp::Min, "a", "b").into());
    against(SdfOp::Intersect, Math::new(MathOp::Max, "a", "b").into());
    against(
        SdfOp::Subtract,
        Math::new(MathOp::Max, "a", Math::new(MathOp::Sub, 0.0, "b")).into(),
    );

    // And a fillet wider than the repeat is refused at the field that asked.
    let refused = MaterialGraph::builder("test:combine")
        .node("n", SdfCombine::new(SdfOp::Union, 0.0, 0.0).smooth(-0.5))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .expect_err("a fillet of negative width");
    assert_eq!(refused.path, "nodes[n].smooth");
}

#[test]
fn a_smooth_union_of_two_touching_discs_is_inside_only_at_the_neck() {
    // Two discs of radius 0.15 whose rims meet at the centre of the repeat.
    // The hard union of them is zero at that one point and positive everywhere
    // else outside the pair; a filleted union pulls a neck of material across
    // it, and the claim is that the neck is the *only* place the two answers
    // differ in sign — everywhere else the fillet is either inside both already
    // or too far from the crease to reach.
    const FILLET: f32 = 0.06;
    let discs = |builder: MaterialGraphBuilder| {
        builder
            .node(
                "left",
                Transform::new(disc_distance(0.15)).translate(-0.15, 0.0),
            )
            .node(
                "right",
                Transform::new(disc_distance(0.15)).translate(0.15, 0.0),
            )
    };
    let ir = wired(|builder| {
        discs(builder)
            .node("hard", SdfCombine::new(SdfOp::Union, "left", "right"))
            .node(
                "soft",
                SdfCombine::new(SdfOp::Union, "left", "right").smooth(FILLET),
            )
            .output(
                PbrOutput::new()
                    .roughness("soft")
                    .metallic("hard")
                    .height("left")
                    .occlusion("right"),
            )
    });
    let read = |uv: [f32; 2]| {
        (
            at(&ir, "roughness", uv)[0],
            at(&ir, "metallic", uv)[0],
            at(&ir, "height", uv)[0],
            at(&ir, "occlusion", uv)[0],
        )
    };

    // At the neck the two distances are both zero, so the blend sits halfway
    // and the fillet is at its deepest: a quarter of the width, exactly.
    let (soft, hard, left, right) = read([0.5, 0.5]);
    assert!(close(left, 0.0) && close(right, 0.0), "{left} {right}");
    assert!(
        close(hard, 0.0),
        "the hard union is zero at the neck: {hard}"
    );
    assert!(close(soft, -FILLET * 0.25), "{soft}");

    let mut necked = 0;
    for j in 0..96_u16 {
        for i in 0..96_u16 {
            let uv = [(f32::from(i) + 0.5) / 96.0, (f32::from(j) + 0.5) / 96.0];
            let (soft, hard, left, right) = read(uv);
            // A fillet only ever adds material, so the union can move inward
            // and never outward.
            assert!(soft <= hard + 1e-6, "at {uv:?}: {soft} against {hard}");
            // Outside the band the two fields are a fillet apart, the ramp
            // clamps and the fillet term vanishes, so the answer is the hard
            // boolean. This is what keeps a rounded joint local: the far rim of
            // either disc is untouched by a fillet at the neck. It is a margin
            // rather than the bits because `mix` is `a + (b - a) * t`, and even
            // at a `t` of exactly one that subtraction and addition round.
            if (left - right).abs() >= FILLET {
                assert!(close(soft, hard), "at {uv:?}: {soft} against {hard}");
            }
            if soft < 0.0 && hard >= 0.0 {
                necked += 1;
                let reach = ((uv[0] - 0.5).powi(2) + (uv[1] - 0.5).powi(2)).sqrt();
                assert!(
                    reach < 0.1,
                    "the fillet reached {uv:?}, {reach} from the neck"
                );
            }
        }
    }
    assert!(necked > 8, "the fillet covered {necked} texels");

    // And the two places that say the same thing by hand: the far rim of the
    // left disc, which the fillet never reaches, and a point a little above the
    // neck, which it does.
    let (soft, hard, ..) = read([0.5 - 0.3, 0.5]);
    assert!(close(soft, hard), "{soft} against {hard}");
    let (soft, hard, ..) = read([0.5, 0.5 + 0.03]);
    assert!(soft < 0.0 && hard > 0.0, "{soft} against {hard}");
}

/// The centre of the crossing of warp thread `i` and weft thread `j`, on a
/// cloth of `x` by `y` threads: the texel where both threads are at their own
/// centreline, which is where a crossing is easiest to read.
fn crossing_at(threads: [f32; 2], i: u16, j: u16) -> [f32; 2] {
    [
        (f32::from(i) + 0.5) / threads[0],
        (f32::from(j) + 0.5) / threads[1],
    ]
}

#[test]
fn a_plain_weaves_warp_is_on_top_exactly_where_its_weft_is_not() {
    // Four threads each way at four fifths of their pitch, so the threads run
    // on 0.25 centres with a twentieth of the repeat of gap either side of
    // each of them.
    let cloth = |output| float_node(Weave::new().x(4).y(4).width(0.8).output(output));
    let ir = wired(|builder| {
        builder
            .node("mask", Weave::new().x(4).y(4).width(0.8))
            .node(
                "warp",
                Weave::new().x(4).y(4).width(0.8).output(WeaveOutput::Warp),
            )
            .node(
                "weft",
                Weave::new().x(4).y(4).width(0.8).output(WeaveOutput::Weft),
            )
            .output(
                PbrOutput::new()
                    .roughness("mask")
                    .metallic("warp")
                    .occlusion("weft"),
            )
    });
    let read = |uv: [f32; 2]| {
        (
            at(&ir, "roughness", uv)[0],
            at(&ir, "metallic", uv)[0],
            at(&ir, "occlusion", uv)[0],
        )
    };

    // A plain weave is a checkerboard of crossings: the warp is over where the
    // two indices have the same parity and under where they do not, and it is
    // one or the other at every crossing rather than something between.
    for j in 0..4_u16 {
        for i in 0..4_u16 {
            let uv = crossing_at([4.0, 4.0], i, j);
            let (mask, warp, weft) = read(uv);
            let over = (i + j) % 2 == 0;
            assert!(close(mask, 1.0), "the crossing at {i},{j} is cloth: {mask}");
            assert!(
                close(warp, f32::from(u8::from(over))) && close(weft, f32::from(u8::from(!over))),
                "at {i},{j}: warp {warp}, weft {weft}"
            );
        }
    }

    // In the gap where two of the gaps cross, nothing is cloth and neither
    // thread is on top: the mask is what says the pair partition the cloth
    // rather than the repeat.
    let (mask, warp, weft) = read([0.0, 0.0]);
    assert!(close(mask, 0.0) && close(warp, 0.0) && close(weft, 0.0));

    // And that is the claim over the whole repeat, at every texel rather than
    // at the crossings alone: the warp is on top exactly where the weft is
    // not, and the two of them together are the cloth.
    let mut seen = [0_u32; 2];
    for row in 0..96_u16 {
        for column in 0..96_u16 {
            let uv = [f32::from(column) / 96.0, f32::from(row) / 96.0];
            let (mask, warp, weft) = read(uv);
            assert!(
                close(warp + weft, mask),
                "at {uv:?}: {warp} + {weft} != {mask}"
            );
            assert!(close(warp * weft, 0.0), "at {uv:?}: both threads on top");
            seen[0] += u32::from(close(warp, 1.0));
            seen[1] += u32::from(close(weft, 1.0));
        }
    }
    // Both halves of the cloth are actually reached, so the equality above is
    // not the equality of two fields that are zero everywhere.
    assert!(seen[0] > 1000 && seen[1] > 1000, "{seen:?}");

    // The mask read on its own is the same field, which is what says the three
    // outputs are three readings of one cloth rather than three cloths.
    let alone = cloth(WeaveOutput::Mask);
    for step in 0..64_u16 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 23) / 32.0];
        assert!(close(float_at(&alone, uv), read(uv).0), "at {uv:?}");
    }
}

#[test]
fn a_weaves_relief_is_continuous_across_the_crossings_it_reads() {
    let relief = float_node(
        Weave::new()
            .x(4)
            .y(4)
            .width(0.8)
            .output(WeaveOutput::Height),
    );
    let height = |uv: [f32; 2]| float_at(&relief, uv);

    // The three levels the model names. At a crossing the thread on top is at
    // its crest, whichever of the two it is; along a thread between two
    // crossings there is nothing on it to lift it and it stands at its own
    // crest; where two gaps cross there is no cloth at all.
    assert!(close(height([0.125, 0.125]), 1.0), "the warp over a weft");
    assert!(close(height([0.375, 0.125]), 1.0), "the weft over a warp");
    assert!(close(height([0.125, 0.25]), 0.75), "along a warp thread");
    assert!(close(height([0.25, 0.125]), 0.75), "along a weft thread");
    assert!(close(height([0.25, 0.25]), 0.0), "the hole between four");

    // And it is continuous, which is the claim the model was built for: a
    // thread going from over to under does it through the gap between the two
    // threads it crosses, where the cross-section carrying the lift has
    // already fallen to zero, so the pattern may change there without the
    // field stepping. A march at a thousandth of the repeat would show a step
    // of a quarter as a jump of a quarter.
    let steps = 1024_u16;
    for line in [0.125_f32, 0.2, 0.375] {
        let mut last = [height([line, 0.0]), height([0.0, line])];
        let mut largest = 0.0_f32;
        for step in 1..=steps {
            let along = f32::from(step) / f32::from(steps);
            let here = [height([line, along]), height([along, line])];
            largest = largest.max((here[0] - last[0]).abs());
            largest = largest.max((here[1] - last[1]).abs());
            last = here;
        }
        // A cosine ridge a fifth of the repeat wide moves by at most about
        // four thousandths over a thousandth of the repeat; a pattern that
        // stepped would move by a quarter.
        assert!(largest < 0.02, "along {line}: a jump of {largest}");
    }

    // The relief stays inside the unit a height map is read in, without a
    // clamp anywhere in the lowering.
    for row in 0..64_u16 {
        for column in 0..64_u16 {
            let uv = [f32::from(column) / 64.0, f32::from(row) / 64.0];
            let value = height(uv);
            assert!((0.0..=1.0).contains(&value), "at {uv:?}: {value}");
        }
    }
}

#[test]
fn a_twills_float_is_half_its_repeat_and_a_satins_binding_points_scatter() {
    // A twill of four threads floats over two of them and moves its float one
    // thread along per weft, which is the diagonal wale of a denim. The rule
    // is read here against the arithmetic written out in Rust, at the centre
    // of every crossing of a cloth of eight threads each way.
    let twill = float_node(
        Weave::new()
            .x(8)
            .y(8)
            .width(0.8)
            .pattern(WeavePattern::Twill { step: 4 })
            .output(WeaveOutput::Warp),
    );
    for j in 0..8_u16 {
        for i in 0..8_u16 {
            let over = (i + 8 - j) % 4 < 2;
            let warp = float_at(&twill, crossing_at([8.0, 8.0], i, j));
            assert!(
                close(warp, f32::from(u8::from(over))),
                "a twill at {i},{j}: {warp}"
            );
        }
    }

    // A satin of five floats over four threads and binds one, and the whole
    // point of it is where those binding points land: exactly one per weft and
    // exactly one per thread, and never in two adjacent wefts at the same
    // thread, which is what keeps the cloth from reading as a diagonal.
    let satin = float_node(
        Weave::new()
            .x(5)
            .y(5)
            .width(0.8)
            .pattern(WeavePattern::Satin { step: 5 })
            .output(WeaveOutput::Weft),
    );
    let bound: Vec<u16> = (0..5_u16)
        .map(|j| {
            let points: Vec<u16> = (0..5_u16)
                .filter(|i| close(float_at(&satin, crossing_at([5.0, 5.0], *i, j)), 1.0))
                .collect();
            assert_eq!(points.len(), 1, "weft {j} binds {points:?}");
            points[0]
        })
        .collect();
    let mut threads = bound.clone();
    threads.sort_unstable();
    assert_eq!(threads, vec![0_u16, 1, 2, 3, 4], "every thread bound once");
    for pair in bound.windows(2) {
        let step = (pair[1] + 5 - pair[0]) % 5;
        assert!(step > 1 && step < 4, "a move of {step} is a twill's move");
    }

    // A per-thread id is one number the length of a thread, a different one on
    // the thread beside it, and the same thread again a whole repeat along.
    let ids = float_node(Weave::new().x(4).y(4).width(0.8).output(WeaveOutput::Id));
    // Along the warp thread at 0.125, between wefts so that the warp is what
    // shows: a thread carries its own number wherever it is read.
    let here = float_at(&ids, [0.125, 0.25]);
    assert!((0.0..=1.0).contains(&here));
    assert!(close(here, float_at(&ids, [0.125, 0.75])), "one thread");
    assert!(!close(here, float_at(&ids, [0.375, 0.25])), "the next one");
    assert!(close(here, float_at(&ids, [1.125, 0.25])), "and the wrap");
    // The weft threads are hashed under their own seed, so the third warp and
    // the third weft are not one number.
    assert!(
        !close(float_at(&ids, [0.625, 0.25]), float_at(&ids, [0.25, 0.625])),
        "warp and weft share a hash"
    );
}

#[test]
fn a_cloth_whose_threads_do_not_divide_its_crossing_is_refused_by_path() {
    let refused = |weave: Weave| {
        MaterialGraph::builder("test:weave")
            .node("n", weave)
            .output(PbrOutput::new().roughness("n"))
            .build()
            .expect_err("a cloth that does not close")
            .path
    };
    // A plain weave crosses one way on even threads and the other way on odd
    // ones, so an odd count would put thread zero's crossing next to its own
    // opposite at the seam.
    assert_eq!(refused(Weave::new().x(7).y(8)), "nodes[n].x");
    assert_eq!(refused(Weave::new().x(8).y(3)), "nodes[n].y");
    // The same rule at the pattern's own repeat rather than at two.
    assert_eq!(
        refused(
            Weave::new()
                .x(8)
                .y(8)
                .pattern(WeavePattern::Twill { step: 3 })
        ),
        "nodes[n].x"
    );
    // Two threads of a twill is the plain weave under another name, and a
    // satin of four or six threads has no move of its own: both are refused at
    // the field that decides them rather than laid as something else.
    assert_eq!(
        refused(Weave::new().pattern(WeavePattern::Twill { step: 2 })),
        "nodes[n].pattern.step"
    );
    assert_eq!(
        refused(
            Weave::new()
                .x(6)
                .y(6)
                .pattern(WeavePattern::Satin { step: 6 })
        ),
        "nodes[n].pattern.step"
    );
    // And a thread that fills its whole pitch leaves no gap for the thread
    // crossing it, which is a plane rather than a cloth.
    assert_eq!(refused(Weave::new().width(1.0)), "nodes[n].width");
    assert_eq!(refused(Weave::new().width(0.0)), "nodes[n].width");

    // A satin of five over ten threads is what does close, in both axes and
    // through every output, which is what says the refusals above are about
    // the rule rather than about the node.
    for output in [
        WeaveOutput::Mask,
        WeaveOutput::Height,
        WeaveOutput::Warp,
        WeaveOutput::Weft,
        WeaveOutput::Id,
    ] {
        MaterialGraph::builder("test:weave")
            .node(
                "n",
                Weave::new()
                    .x(10)
                    .y(5)
                    .pattern(WeavePattern::Satin { step: 5 })
                    .output(output),
            )
            .output(PbrOutput::new().roughness("n"))
            .build()
            .expect("a satin that closes");
    }
}

#[test]
fn a_transform_reads_its_source_through_the_frame_it_names() {
    let through = |transform: Transform| {
        wired(|builder| {
            builder
                .node("src", Noise::value().period(4).seed(1))
                .node("n", transform)
                .output(PbrOutput::new().roughness("n"))
        })
    };
    let plain = float_node(Noise::value().period(4).seed(1));

    // A scale of two reads the source twice across the repeat.
    let scaled = through(Transform::new("src").scale(2.0));
    for uv in [[0.1_f32, 0.2_f32], [0.43, 0.77], [0.0, 0.0]] {
        let expected = float_at(&plain, [(uv[0] * 2.0).fract(), (uv[1] * 2.0).fract()]);
        let value = float_at(&scaled, uv);
        assert!(close(value, expected), "at {uv:?}: {value} not {expected}");
    }

    // A quarter turn about the centre of the repeat exchanges the axes:
    // `out(u, v) = in(1 - v, u)`, which is the rotation the period rule
    // transposes a period for.
    let turned = through(Transform::new("src").rotate(90.0));
    for uv in [[0.1_f32, 0.2_f32], [0.43, 0.77]] {
        let expected = float_at(&plain, [(1.0 - uv[1]).fract(), uv[0]]);
        let value = float_at(&turned, uv);
        assert!(close(value, expected), "at {uv:?}: {value} not {expected}");
    }

    // A translation moves the frame and nothing else.
    let moved = through(Transform::new("src").translate(0.25, 0.5));
    let expected = float_at(&plain, [(0.1 + 0.25_f32).fract(), (0.2 + 0.5_f32).fract()]);
    assert!(close(float_at(&moved, [0.1, 0.2]), expected));

    // And the three compose in the order the node documents: scale, then the
    // turn, then the translation.
    let all = through(
        Transform::new("src")
            .scale(2.0)
            .rotate(90.0)
            .translate(0.1, 0.0),
    );
    let uv = [0.3_f32, 0.6_f32];
    let scaled_uv = [uv[0] * 2.0, uv[1] * 2.0];
    let spun = [0.5 - (scaled_uv[1] - 0.5), 0.5 + (scaled_uv[0] - 0.5)];
    let expected = float_at(&plain, [(spun[0] + 0.1).fract(), spun[1].fract()]);
    let value = float_at(&all, uv);
    assert!(close(value, expected), "{value} not {expected}");
}

#[test]
fn a_warp_reads_its_source_at_a_coordinate_the_offset_moved() {
    // A constant offset moves the whole frame by a known amount, which is the
    // one warp whose answer can be written down.
    let ir = wired(|builder| {
        builder
            .node(
                "n",
                Warp::new("u", Input::Const(Value::Vec2([0.2, 0.0]))).amount(0.5),
            )
            .output(PbrOutput::new().roughness("n"))
    });
    // The source is the u coordinate itself, so the answer is the coordinate
    // the warp read it at: `fract(u + 0.5 * 0.2)`.
    assert!(close(float_at(&ir, [0.3, 0.0]), 0.4));
    assert!(close(float_at(&ir, [0.95, 0.0]), 0.05), "and it wraps");

    // A float offset displaces along both axes, which is what the port's
    // conversion means.
    let both = wired(|builder| {
        builder
            .node("n", Warp::new("v", 0.25).amount(1.0))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&both, [0.0, 0.5]), 0.75));

    // The offset is read at the texel's own coordinate and the source at the
    // moved one, which is what makes this a distortion rather than a blend.
    let field = wired(|builder| {
        builder
            .node("n", Warp::new("u", "v").amount(0.5))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&field, [0.1, 0.4]), 0.3));
}

#[test]
fn a_mirror_folds_the_whole_unit_and_a_kaleidoscope_folds_the_quadrants() {
    // The fold is `0.5 - |0.5 - fract(t)|`, so the second half of the axis is
    // the first half reversed and the two sides of the seam are one texel.
    let mirrored = wired(|builder| {
        builder
            .node("n", Mirror::new("u"))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&mirrored, [0.3, 0.0]), 0.3));
    assert!(close(float_at(&mirrored, [0.7, 0.0]), 0.3));
    assert!(close(float_at(&mirrored, [0.5, 0.0]), 0.5));
    assert!(close(float_at(&mirrored, [0.0, 0.0]), 0.0));
    assert!(close(float_at(&mirrored, [1.0, 0.0]), 0.0), "the seam");

    // The other axis is untouched, which is why a mirror of a free field is
    // still free.
    let sideways = wired(|builder| {
        builder
            .node("n", Mirror::new("v").axis(MirrorAxis::V))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&sideways, [0.0, 0.8]), 0.2));

    // The quadrant fold runs from zero to one and back within the unit, so it
    // reads the source's whole range twice per axis.
    let folded = wired(|builder| {
        builder
            .node("n", Kaleidoscope::new("u"))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&folded, [0.25, 0.0]), 0.5));
    assert!(close(float_at(&folded, [0.5, 0.0]), 1.0));
    assert!(close(float_at(&folded, [0.75, 0.0]), 0.5));
    assert!(close(float_at(&folded, [0.0, 0.0]), 0.0));
    assert!(close(float_at(&folded, [1.0, 0.0]), 0.0), "the seam");

    // Any other count is the angular fold, which is a different construction
    // and does not tile: it validates, it lowers, and its period is free, so
    // the output has to be reached through something that tiles again.
    let material = MaterialGraph::builder("test:wedges")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("n", Kaleidoscope::new("u").count(6))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(material.path, "nodes[n]");
    let wedges = wired(|builder| {
        builder
            .node("k", Kaleidoscope::new("u").count(6))
            // A tile wraps whatever it scattered, so a free field reaches the
            // output through one.
            .node("n", Tile::new("k").count(1))
            .output(PbrOutput::new().roughness("n"))
    });
    // Sector zero is read as it stands, and the sector next to it is that one
    // reflected: two directions the same angle either side of a sector edge
    // read the same value.
    let sector = std::f32::consts::TAU / 6.0;
    let sample = |angle: f32| float_at(&wedges, [0.5 + 0.2 * angle.cos(), 0.5 + 0.2 * angle.sin()]);
    assert!(close(sample(sector * 0.25), sample(-sector * 0.25)));
    assert!(close(sample(sector * 0.75), sample(sector * 1.25)));
}

#[test]
fn a_circle_map_reads_its_source_round_a_disc_and_fills_outside_it() {
    // A `Uv` v axis through the map is the normalised radius itself, so along
    // any spoke the field climbs from nothing at the hole to one at the rim.
    let radial = wired(|builder| {
        builder
            .node(
                "n",
                CircleMap::new("v").radius(0.5).inner(0.1).outside(0.125),
            )
            .output(PbrOutput::new().roughness("n"))
    });
    let spoke = |radius: f32| float_at(&radial, [0.5 + radius, 0.5]);
    for (radius, expected) in [(0.2_f32, 0.25_f32), (0.3, 0.5), (0.4, 0.75), (0.45, 0.875)] {
        assert!(
            close(spoke(radius), expected),
            "at radius {radius}: {} rather than {expected}",
            spoke(radius)
        );
    }
    // And it climbs, rather than only landing on those four numbers: the ramp
    // is monotone in radius, which is what makes the node a resampler of the
    // source's v axis rather than a fold of it.
    let mut previous = f32::NEG_INFINITY;
    for step in 0..32 {
        let radius = 0.12 + 0.01 * f32::from(u8::try_from(step).unwrap());
        let value = spoke(radius);
        assert!(
            value > previous,
            "at radius {radius}: {value} after {previous}"
        );
        previous = value;
    }
    // Inside the hole and past the rim there is no source to read, and the
    // answer is the fill rather than whatever the source would have said.
    assert!(close(spoke(0.05), 0.125), "inside the hole");
    assert!(close(float_at(&radial, [0.0, 0.0]), 0.125), "past the rim");

    // The fill defaults to nothing, which is what a mask wants.
    let plain = wired(|builder| {
        builder
            .node("n", CircleMap::new("v").radius(0.4))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&plain, [0.0, 0.0]), 0.0), "past the rim");
    assert!(
        close(float_at(&plain, [0.5 + 0.2, 0.5]), 0.5),
        "half way out"
    );
}

#[test]
fn eight_stripes_through_a_circle_map_are_eight_spokes() {
    // The source's u axis goes round the ring, so a wave that repeats eight
    // times across the source repeats eight times round the circle. The
    // pattern's v axis is one repeat and its two axes multiply, so the samples
    // are taken past half the window's width, where that factor is one.
    let spokes = wired(|builder| {
        builder
            .node("stripes", Pattern::new(PatternKind::Stripes).x(8).y(1))
            .node("n", CircleMap::new("stripes").radius(0.45))
            .output(PbrOutput::new().roughness("n"))
    });
    let samples: Vec<f32> = (0..64)
        .map(|index| {
            // Half a step off every stripe edge, so what is being checked is
            // where the spokes are rather than how a step rounds.
            let angle =
                std::f32::consts::TAU * (f32::from(u8::try_from(index).unwrap()) + 0.5) / 64.0;
            float_at(
                &spokes,
                [0.5 + 0.36 * angle.cos(), 0.5 + 0.36 * angle.sin()],
            )
        })
        .collect();
    for value in &samples {
        assert!(
            close(*value, 0.0) || close(*value, 1.0),
            "{value} is neither"
        );
    }
    let rising = samples
        .iter()
        .enumerate()
        .filter(|(index, value)| {
            let before = samples[(index + samples.len() - 1) % samples.len()];
            **value > 0.5 && before < 0.5
        })
        .count();
    assert_eq!(rising, 8, "spokes round the circle: {samples:?}");
}

#[test]
fn a_twist_of_one_turn_carries_the_spoke_a_whole_turn_across_the_window() {
    // The source is its own u axis, so what the field answers *is* the angle
    // the map read the source at, in turns.
    let of = |twist: f32| {
        wired(move |builder| {
            builder
                .node("n", CircleMap::new("u").radius(0.4).twist(twist))
                .output(PbrOutput::new().roughness("n"))
        })
    };
    // Untwisted, a spoke is a spoke: the whole ray at angle zero reads the
    // source at u = 0, however far out it is.
    let straight = of(0.0);
    for radius in [0.0_f32, 0.1, 0.2, 0.3] {
        let value = float_at(&straight, [0.5 + radius, 0.5]);
        assert!(close(value, 0.0), "at radius {radius}: {value}");
    }
    // Twisted by one turn, the same ray sweeps the source's whole u axis
    // between the centre and the rim: a quarter of the way out is a quarter
    // turn along, which is the spoke moved a quarter turn round.
    let twisted = of(1.0);
    for (radius, expected) in [(0.0_f32, 0.0_f32), (0.1, 0.25), (0.2, 0.5), (0.3, 0.75)] {
        let value = float_at(&twisted, [0.5 + radius, 0.5]);
        assert!(close(value, expected), "at radius {radius}: {value}");
    }
}

#[test]
fn four_discs_round_a_circle_splatter_are_four_maxima_at_the_compass_points() {
    // Four instances round one ring sit at whole slots of a quarter turn, and
    // slot zero is the +u direction, so a splatter of four is a splatter at the
    // compass points. That is the claim the rest of the node hangs off: the
    // angle a texel is at names the slot it is nearest to, and the instance in
    // that slot is drawn where that slot's own angle put it.
    let splattered = wired(|builder| {
        builder
            .node("dot", Shape::new(ShapeKind::Circle).size(0.45).edge(0.0))
            .node(
                "n",
                CircleSplatter::new("dot").count(4).radius(0.3).scale(0.25),
            )
            .output(PbrOutput::new().roughness("n"))
    });
    for compass in [[1.0_f32, 0.0_f32], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]] {
        let at = [0.5 + 0.3 * compass[0], 0.5 + 0.3 * compass[1]];
        assert!(
            close(float_at(&splattered, at), 1.0),
            "at {compass:?}: {}",
            float_at(&splattered, at)
        );
    }
    // And nowhere else. The middle is the one place a ring cannot reach, which
    // is what makes this node the bolt circle rather than a scatter: every
    // instance stands `radius` out, so the centre is outside all four of them.
    assert!(close(float_at(&splattered, [0.5, 0.5]), 0.0), "the centre");

    // Round the ring itself there are four instances and four gaps, counted by
    // walking it rather than by landing on the four points that were asked for.
    let samples: Vec<f32> = (0..64)
        .map(|index| {
            let angle =
                std::f32::consts::TAU * (f32::from(u8::try_from(index).unwrap()) + 0.5) / 64.0;
            float_at(
                &splattered,
                [0.5 + 0.3 * angle.cos(), 0.5 + 0.3 * angle.sin()],
            )
        })
        .collect();
    let rising = samples
        .iter()
        .enumerate()
        .filter(|(index, value)| {
            let before = samples[(index + samples.len() - 1) % samples.len()];
            **value > 0.5 && before < 0.5
        })
        .count();
    assert_eq!(rising, 4, "instances round the ring: {samples:?}");
}

#[test]
fn a_circle_splatter_mask_keeps_an_instance_whole_or_not_at_all() {
    // The same gate a scatter uses, on a ring instead of a grid: the mask is
    // read once per instance, at that instance's own centre, and compared with
    // a hash of the instance. So a mask of a half keeps about half the bolts at
    // full strength rather than dimming all eight of them — bolts missing read
    // as bolts missing, and bolts at half brightness read as a mistake.
    let drawn = |mask: f32| {
        let splattered = wired(move |builder| {
            builder
                .node("dot", Shape::new(ShapeKind::Circle).size(0.45).edge(0.0))
                .node(
                    "n",
                    CircleSplatter::new("dot")
                        .count(8)
                        .radius(0.3)
                        .scale(0.15)
                        .mask(mask)
                        .seed(5),
                )
                .output(PbrOutput::new().roughness("n"))
        });
        (0_u8..8)
            .map(|slot| {
                let angle = std::f32::consts::TAU * f32::from(slot) / 8.0;
                let value = float_at(
                    &splattered,
                    [0.5 + 0.3 * angle.cos(), 0.5 + 0.3 * angle.sin()],
                );
                assert!(
                    close(value, 0.0) || close(value, 1.0),
                    "an instance came out at {value}, which is neither drawn nor not"
                );
                close(value, 1.0)
            })
            .collect::<Vec<bool>>()
    };

    // A literal one is the default, and the lowering emits no gate for it at
    // all, so this is also the claim that an unmasked ring is the ring it was
    // before the port existed.
    assert!(
        drawn(1.0).iter().all(|kept| *kept),
        "a mask of one keeps every one"
    );
    // A hash is in `0..=1`, and nothing is above zero but zero.
    assert!(
        drawn(0.0).iter().all(|kept| !*kept),
        "a mask of zero kept an instance"
    );
    let half = drawn(0.5);
    let count = half.iter().filter(|kept| **kept).count();
    assert!(
        (2..=7).contains(&count),
        "a mask of a half kept {count} of eight"
    );
    // And the ones it kept are the ones with the smaller hashes, so raising the
    // mask only ever adds instances rather than trading one for another.
    for (low, high) in half.iter().zip(drawn(0.9)) {
        assert!(!low || high, "an instance the fuller mask dropped");
    }
}

#[test]
fn a_circle_splatters_rings_stand_between_its_radii_and_face_the_centre_on_request() {
    // Two rings cut `inner..=radius` into two bands and stand at the outer edge
    // of each, so the outermost is at `radius` whatever `rings` is: a band of
    // 0.1 to 0.4 in two puts them at 0.25 and 0.4. Nothing stands at `inner`
    // itself, which is why it is a standoff rather than a ring.
    let rings = wired(|builder| {
        builder
            .node("dot", Shape::new(ShapeKind::Circle).size(0.45).edge(0.0))
            .node(
                "n",
                CircleSplatter::new("dot")
                    .count(4)
                    .rings(2)
                    .inner(0.1)
                    .radius(0.4)
                    .scale(0.1),
            )
            .output(PbrOutput::new().roughness("n"))
    });
    let east = |radius: f32| float_at(&rings, [0.5 + radius, 0.5]);
    for (radius, drawn) in [
        (0.1_f32, false),
        (0.25, true),
        (0.325, false),
        (0.4, true),
        (0.45, false),
    ] {
        let value = east(radius);
        assert_eq!(
            close(value, 1.0),
            drawn,
            "at radius {radius}: {value} where {drawn} was wanted"
        );
    }

    // Facing the centre turns each instance's own frame by its own angle, so
    // the source's u axis runs outward along the radius instead of along the
    // repeat's u. Read through a source that *is* its own u axis, the
    // difference is which way a step away from an instance's centre reads.
    let of = |facing: bool| {
        wired(move |builder| {
            let mut node = CircleSplatter::new("u").count(4).radius(0.3).scale(0.2);
            if facing {
                node = node.face_centre();
            }
            builder
                .node("n", node)
                .output(PbrOutput::new().roughness("n"))
        })
    };
    // The instance at the north compass point, at (0.5, 0.8), reads the middle
    // of its source either way: a centre is a centre.
    let square = of(false);
    let facing = of(true);
    assert!(close(float_at(&square, [0.5, 0.8]), 0.5), "square, centred");
    assert!(close(float_at(&facing, [0.5, 0.8]), 0.5), "facing, centred");
    // Square to the repeat, a quarter of an instance along the repeat's u is a
    // quarter along the source's u; radially outward is along its v and the u
    // it reads does not move.
    assert!(
        close(float_at(&square, [0.55, 0.8]), 0.75),
        "square, along u"
    );
    assert!(
        close(float_at(&square, [0.5, 0.85]), 0.5),
        "square, outward"
    );
    // Facing the centre, the two swap: outward is the source's u now.
    assert!(
        close(float_at(&facing, [0.55, 0.8]), 0.5),
        "facing, along u"
    );
    assert!(
        close(float_at(&facing, [0.5, 0.85]), 0.75),
        "facing, outward"
    );
}

#[test]
fn a_circle_splatter_measures_a_turned_instance_by_its_corner() {
    // What an instance draws is the source's unit *square*, so how far it
    // reaches from its own centre is not one number: half its width while it
    // stands square to the repeat, and its half-diagonal — a width over the
    // root of two — once `face_centre` or a rotation variation turns it. A
    // bound written against the half-width alone admits a ring whose corners
    // hang past the repeat, and the seam takes them off.
    let ring = |node: CircleSplatter| {
        MaterialGraph::builder("test:ring")
            // A source that fills its own unit, because the claim here is
            // about the square an instance is drawn in rather than about
            // whatever is drawn inside it.
            .node("src", Shape::new(ShapeKind::Box).size(0.5).edge(0.0))
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
            .build()
    };
    let wide = || CircleSplatter::new("src").count(8).radius(0.34).scale(0.3);
    // Square to the repeat this ring reaches 0.49 and is allowed.
    ring(wide()).expect("half a width past the ring is inside the repeat");
    // Turned, the same ring reaches 0.55 and is refused at the field that
    // decides it, in words that say which reach was measured.
    for (turn, node) in [
        ("a spin of its own", wide().rotation_variation(1.0).seed(3)),
        ("a turn to the centre", wide().face_centre()),
    ] {
        let error = ring(node).expect_err(turn);
        assert_eq!(error.path, "nodes[n].radius", "{turn}");
        assert!(error.reason.contains("corner"), "{turn}: {}", error.reason);
    }

    // And what the bound does admit is whole. The widest turned ring it allows
    // at this radius is 0.21 across; drawn from a source that fills its unit,
    // it lights nothing at either seam, whichever way its instances came out.
    for (turn, node) in [
        (
            "a spin of its own",
            wide().scale(0.21).rotation_variation(1.0).seed(3),
        ),
        ("a turn to the centre", wide().scale(0.21).face_centre()),
    ] {
        let material = ring(node).expect("a ring inside the bound");
        let ir = lower(&material, Target::Bake).unwrap();
        for step in 0_u16..=256 {
            let along = f32::from(step) / 256.0;
            for at in [[0.0, along], [along, 0.0]] {
                let value = float_at(&ir, at);
                assert!(
                    close(value, 0.0),
                    "{turn}: an instance reached the seam at {at:?}, where it reads {value}"
                );
            }
        }
    }
}

#[test]
fn a_tile_scatters_its_source_into_every_cell_of_the_repeat() {
    // A circle scattered two by two: each cell holds the whole shape, shrunk
    // into it, so the middle of a cell is the middle of a circle and the
    // corner where four cells meet is outside every one of them.
    let scattered = wired(|builder| {
        builder
            .node("dot", Shape::new(ShapeKind::Circle).size(0.4).edge(0.0))
            .node("n", Tile::new("dot").count(2))
            .output(PbrOutput::new().roughness("n"))
    });
    for centre in [
        [0.25_f32, 0.25_f32],
        [0.75, 0.25],
        [0.25, 0.75],
        [0.75, 0.75],
    ] {
        assert!(close(float_at(&scattered, centre), 1.0), "at {centre:?}");
    }
    assert!(
        close(float_at(&scattered, [0.0, 0.0]), 0.0),
        "a cell corner"
    );
    assert!(close(float_at(&scattered, [0.5, 0.5]), 0.0), "and another");

    // An instance is drawn only where the coordinate it was read at lands
    // inside the source's own unit. Without that window the source, which
    // repeats, would be drawn over the whole cell rather than in it: a ramp
    // scattered four ways is four ramps, each ending where its cell does.
    let ramps = wired(|builder| {
        builder
            .node("n", Tile::new("u").count(4))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&ramps, [0.125, 0.5]), 0.5), "halfway up one");
    assert!(close(float_at(&ramps, [0.375, 0.5]), 0.5), "and the next");

    // The variations are read off the instance's own cell, so two cells differ
    // and one cell is itself a repeat along.
    let varied = wired(|builder| {
        builder
            .node("dot", Shape::new(ShapeKind::Circle).size(0.45).edge(0.05))
            .node("n", Tile::new("dot").count(2).scale_variation(0.8).seed(4))
            .output(PbrOutput::new().roughness("n"))
    });
    let across = |centre: f32| {
        (0_u16..32)
            .map(|step| float_at(&varied, [centre + f32::from(step) / 128.0, 0.25]))
            .collect::<Vec<_>>()
    };
    let first = across(0.25);
    let second = across(0.75);
    assert!(
        first.iter().zip(&second).any(|(l, r)| !close(*l, *r)),
        "two instances of one source came out the same size: {first:?}"
    );
    assert!(
        first.iter().zip(across(1.25)).all(|(l, r)| close(*l, r)),
        "and one instance is itself a repeat along"
    );
}

#[test]
fn a_directional_warp_moves_one_fixed_step_the_way_its_angle_field_points() {
    // The angle is in turns, so a constant of a quarter is a quarter turn: the
    // frame moves along +v, and the source — the v coordinate itself — answers
    // the coordinate it was read at.
    let up = wired(|builder| {
        builder
            .node("n", DirectionalWarp::new("v", 0.25).amount(0.2))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&up, [0.0, 0.3]), 0.5));
    assert!(close(float_at(&up, [0.0, 0.9]), 0.1), "and it wraps");

    // A zero angle points along +u, and the displacement is the amount whatever
    // the field's own values are — which is what makes this a flow rather than
    // a squeeze: two texels a hair apart in the angle field move by the same
    // distance in slightly different directions, so a line through them bends
    // instead of pinching.
    let along = wired(|builder| {
        builder
            .node("n", DirectionalWarp::new("u", 0.0).amount(0.25))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&along, [0.5, 0.0]), 0.75));

    // Half a turn is the other way, and is the same distance.
    let back = wired(|builder| {
        builder
            .node("n", DirectionalWarp::new("u", 0.5).amount(0.25))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&back, [0.5, 0.0]), 0.25));
}

#[test]
fn a_height_to_mask_is_one_inside_its_band_and_ramps_across_its_bounds() {
    // A hard band first: with no softness the two smoothsteps degenerate to
    // steps, which is the case that rests on the crate's own guarded
    // smoothstep answering something for edges that meet.
    let hard = wired(|builder| {
        builder
            .node("n", HeightToMask::band("u", 0.25, 0.75).softness(0.0))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&hard, [0.5, 0.0]), 1.0), "inside");
    assert!(close(float_at(&hard, [0.1, 0.0]), 0.0), "below");
    assert!(close(float_at(&hard, [0.9, 0.0]), 0.0), "above");

    // And a soft one: the ramp is centred on the bound, so the bound itself is
    // a half and the band is the width it was authored at however soft it is.
    let soft = wired(|builder| {
        builder
            .node("n", HeightToMask::band("u", 0.25, 0.75).softness(0.2))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&soft, [0.25, 0.0]), 0.5), "the lower bound");
    assert!(close(float_at(&soft, [0.75, 0.0]), 0.5), "the upper bound");
    assert!(close(float_at(&soft, [0.5, 0.0]), 1.0), "the middle");
    assert!(close(float_at(&soft, [0.1, 0.0]), 0.0), "well below");

    // The two shortcuts are the band with one bound at the end of the
    // interval, which is what "above" and "below" mean for a `0..=1` field.
    let above = wired(|builder| {
        builder
            .node("n", HeightToMask::above("u", 0.5).softness(0.0))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&above, [0.9, 0.0]), 1.0));
    assert!(close(float_at(&above, [0.1, 0.0]), 0.0));
    let below = wired(|builder| {
        builder
            .node("n", HeightToMask::below("u", 0.5).softness(0.0))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&below, [0.1, 0.0]), 1.0));
    assert!(close(float_at(&below, [0.9, 0.0]), 0.0));
}

#[test]
fn a_tile_mask_keeps_an_instance_whole_or_not_at_all() {
    // Four by four dots, gated by a constant mask. The gate is a threshold
    // against each instance's own hash, so what a mask of a half does is keep
    // about half the instances at full strength — not dim all sixteen. That is
    // the difference between gravel thinning out and gravel dissolving.
    let drawn = |mask: f32| {
        let scattered = wired(|builder| {
            builder
                .node("dot", Shape::new(ShapeKind::Circle).size(0.4).edge(0.0))
                .node("n", Tile::new("dot").count(4).mask(mask).seed(7))
                .output(PbrOutput::new().roughness("n"))
        });
        let mut kept = Vec::new();
        for row in 0_u8..4 {
            for column in 0_u8..4 {
                let centre = [
                    (f32::from(column) + 0.5) / 4.0,
                    (f32::from(row) + 0.5) / 4.0,
                ];
                let value = float_at(&scattered, centre);
                assert!(
                    close(value, 0.0) || close(value, 1.0),
                    "an instance came out at {value}, which is neither drawn nor not"
                );
                kept.push(close(value, 1.0));
            }
        }
        kept
    };

    // A literal one is the default, and the lowering emits no gate for it at
    // all, so this is also the claim that a scatter nobody masked is the
    // scatter it was before the port existed.
    assert!(
        drawn(1.0).iter().all(|kept| *kept),
        "a mask of one keeps every one"
    );
    // A hash is in `0..=1`, so a mask of zero clears the whole lattice: an
    // instance is kept where the mask is at or above its own number, and
    // nothing is above zero but zero.
    assert!(
        drawn(0.0).iter().all(|kept| !*kept),
        "a mask of zero kept an instance"
    );
    let half = drawn(0.5);
    let count = half.iter().filter(|kept| **kept).count();
    assert!(
        (4..=12).contains(&count),
        "a mask of a half kept {count} of sixteen"
    );
    // And the ones it kept are the ones with the smaller hashes, so raising the
    // mask only ever adds instances rather than trading one for another.
    for (low, high) in half.iter().zip(drawn(0.9)) {
        assert!(!low || high, "an instance the fuller mask dropped");
    }
}

#[test]
fn a_tile_mask_is_read_at_the_cell_rather_than_at_the_texel() {
    // The mask decides per instance, at the centre of its own cell, so an
    // instance is drawn or not as a whole and a mask finer than the cells does
    // not cut one in half. A ramp across two cells therefore gates the two
    // cells by the two numbers at their centres — 0.25 and 0.75 — and nothing
    // in between.
    let scattered = wired(|builder| {
        builder
            .node("dot", Shape::new(ShapeKind::Circle).size(0.45).edge(0.0))
            .node("n", Tile::new("dot").counts(2, 1).mask("u").seed(11))
            .output(PbrOutput::new().roughness("n"))
    });
    for column in [0.25_f32, 0.75] {
        let across: Vec<f32> = (0_u16..8)
            .map(|step| float_at(&scattered, [column + f32::from(step) / 64.0 - 0.06, 0.5]))
            .collect();
        let first = across.first().copied().unwrap_or_default();
        assert!(
            across.iter().all(|value| close(*value, first)),
            "the instance at {column} was cut by its own mask: {across:?}"
        );
    }
}

#[test]
fn a_scratch_is_bright_on_its_own_segment_and_dark_off_it() {
    // One segment, so the whole field is that segment and the test can put it
    // where the hash put it. With no spread the angle is the mean exactly.
    const SALT: u32 = 0x9E37_79B9;
    let seed = 3;
    let scratches = Scratches::new()
        .count(1)
        .length(0.2)
        .width(0.02)
        .angle(0.0)
        .angle_spread(0.0)
        .seed(seed);
    let ir = float_node(scratches);
    // One cell across the repeat, so the centre is the cell's own two hashes.
    let centre = [hash2(0, 0, seed), hash2(0, 0, seed.wrapping_add(SALT))];
    assert!(close(float_at(&ir, centre), 1.0), "the middle of the line");
    // Along it, within half its length, still the line.
    assert!(close(float_at(&ir, [centre[0] + 0.05, centre[1]]), 1.0));
    // Past the end of it, nothing.
    assert!(close(float_at(&ir, [centre[0] + 0.15, centre[1]]), 0.0));
    // Across it, the width is where it has faded to nothing and half of it is
    // where the fade starts.
    assert!(close(float_at(&ir, [centre[0], centre[1] + 0.01]), 1.0));
    assert!(close(float_at(&ir, [centre[0], centre[1] + 0.02]), 0.0));
    // And it is level, because the mean angle is zero and nothing turned it.
    assert!(close(float_at(&ir, [centre[0] + 0.09, centre[1]]), 1.0));

    // A count past what the lattice holds leaves those cells empty: four
    // scratches on a two-by-two grid is four, and three is three.
    let drawn = |count: u32| {
        let ir = float_node(
            Scratches::new()
                .count(count)
                .length(0.1)
                .width(0.01)
                .seed(1),
        );
        let mut lit = 0_usize;
        for y in 0_u16..128 {
            for x in 0_u16..128 {
                if float_at(&ir, [f32::from(x) / 128.0, f32::from(y) / 128.0]) > 0.5 {
                    lit += 1;
                }
            }
        }
        lit
    };
    let three = drawn(3);
    let four = drawn(4);
    assert!(three > 0 && four > three, "{three} and {four}");

    // A scratch too long for its own lattice is refused rather than searching
    // a neighbourhood no texel can afford.
    let error = MaterialGraph::builder("test:long")
        .node("n", Scratches::new().count(256).length(0.9))
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[n].length");
}

#[test]
fn a_curve_passes_through_its_points_and_never_turns_back() {
    let ir = float_node(Curve::new("u").points([[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]]));
    // The control points are on the curve exactly, which is what makes this a
    // curve *through* them.
    assert!(close(float_at(&ir, [0.0, 0.0]), 0.0));
    assert!(close(float_at(&ir, [0.5, 0.0]), 0.8));
    assert!(close(float_at(&ir, [1.0, 0.0]), 1.0));
    // Past the ends the ends hold, the way a lookup's do.
    assert!(close(float_at(&ir, [-0.5, 0.0]), 0.0));
    assert!(close(float_at(&ir, [1.5, 0.0]), 1.0));

    // Monotone is the whole point: the control points only rise, so the curve
    // never dips between them, which is what a plain cubic through the same
    // points would do at this one's second segment.
    let mut previous = f32::NEG_INFINITY;
    for step in 0_u16..=256 {
        let value = float_at(&ir, [f32::from(step) / 256.0, 0.0]);
        assert!(
            value >= previous - 1e-6,
            "at {step}: {value} after {previous}"
        );
        previous = value;
    }
    // And it is not merely the straight lines between them: halfway up the
    // first segment a linear ramp would answer 0.4.
    assert!(!close(float_at(&ir, [0.25, 0.0]), 0.4));

    // A curve over points that fall runs the other way and stays monotone.
    let falling = float_node(Curve::new("u").points([[0.0, 1.0], [0.4, 0.2], [1.0, 0.0]]));
    let mut previous = f32::INFINITY;
    for step in 0_u16..=256 {
        let value = float_at(&falling, [f32::from(step) / 256.0, 0.0]);
        assert!(
            value <= previous + 1e-6,
            "at {step}: {value} after {previous}"
        );
        previous = value;
    }
    assert!(close(float_at(&falling, [0.4, 0.0]), 0.2));
}

#[test]
fn an_adjust_turns_the_hue_about_the_grey_axis_and_leaves_grey_alone() {
    let adjusted = |node: Adjust| {
        wired(|builder| {
            builder
                .node("c", Combine::new("u", 0.25, 0.75))
                .node("n", node)
                .output(PbrOutput::new().base_color("n"))
        })
    };
    // Grey is the axis a hue rotation turns about, so it does not move.
    let grey = wired(|builder| {
        builder
            .node("n", Adjust::new(0.4).hue(90.0))
            .output(PbrOutput::new().base_color("n"))
    });
    let colour = colour_at(&grey, [0.0, 0.0]);
    assert!(close3(colour, [0.4, 0.4, 0.4]), "{colour:?}");

    // A colour does move, and a whole turn brings it back.
    let turned = colour_at(&adjusted(Adjust::new("c").hue(120.0)), [0.5, 0.0]);
    assert!(!close3(turned, [0.5, 0.25, 0.75]), "{turned:?}");
    let round_trip = colour_at(&adjusted(Adjust::new("c").hue(360.0)), [0.5, 0.0]);
    assert!(close3(round_trip, [0.5, 0.25, 0.75]), "{round_trip:?}");
    // A third of a turn about `(1, 1, 1)` is a cycle of the channels.
    assert!(close3(turned, [0.75, 0.5, 0.25]), "{turned:?}");

    // No saturation is luminance in every channel.
    let grey = colour_at(&adjusted(Adjust::new("c").saturation(0.0)), [0.5, 0.0]);
    let luminance = ashlar_material::luminance([0.5, 0.25, 0.75]);
    assert!(close3(grey, [luminance; 3]), "{grey:?}");

    // Contrast scales about mid grey, and brightness is added after it.
    let contrast = colour_at(&adjusted(Adjust::new("c").contrast(2.0)), [0.5, 0.0]);
    assert!(close3(contrast, [0.5, 0.0, 1.0]), "{contrast:?}");
    let brightened = colour_at(
        &adjusted(Adjust::new("c").contrast(2.0).brightness(0.1)),
        [0.5, 0.0],
    );
    assert!(close3(brightened, [0.6, 0.1, 1.1]), "{brightened:?}");

    // A float input broadcasts on the way in, because hue and saturation have
    // no meaning on one channel.
    let broadcast = colour_at(
        &wired(|builder| {
            builder
                .node("n", Adjust::new("u").brightness(0.25))
                .output(PbrOutput::new().base_color("n"))
        }),
        [0.5, 0.0],
    );
    assert!(close3(broadcast, [0.75; 3]), "{broadcast:?}");
}

#[test]
fn a_clamp_and_a_switch_are_the_two_that_choose() {
    let held = float_node(Clamp::new("u").range(0.25, 0.75));
    assert!(close(float_at(&held, [0.1, 0.0]), 0.25));
    assert!(close(float_at(&held, [0.5, 0.0]), 0.5));
    assert!(close(float_at(&held, [0.9, 0.0]), 0.75));

    // A bound can be a field of its own.
    let ramped = float_node(Clamp::new(0.5).range(0.0, "u"));
    assert!(close(float_at(&ramped, [0.3, 0.0]), 0.3));
    assert!(close(float_at(&ramped, [0.8, 0.0]), 0.5));

    // The switch takes its true branch at a half and above, which is what a
    // `Bool` parameter arriving as zero or one means.
    let chosen = float_node(Switch::new("u", 0.75, 0.25));
    assert!(close(float_at(&chosen, [0.49, 0.0]), 0.25));
    assert!(close(float_at(&chosen, [0.5, 0.0]), 0.75));

    // And it chooses the whole value rather than channel by channel.
    let colours = wired(|builder| {
        builder
            .node("n", Switch::new("u", [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]))
            .output(PbrOutput::new().base_color("n"))
    });
    assert!(close3(colour_at(&colours, [0.9, 0.0]), [1.0, 0.0, 0.0]));
    assert!(close3(colour_at(&colours, [0.1, 0.0]), [0.0, 0.0, 1.0]));
}

#[test]
fn a_normal_from_height_is_the_normal_the_bake_derives_from_the_same_height() {
    use std::collections::BTreeMap;

    use ashlar_material::bake::{BakeRequest, rasterise};

    // One height field, read two ways: bound as the material's height, which
    // is where the bake derives its own normal, and through the node, which is
    // where a graph that wants to blend normals reads one. They are the same
    // derivation over the same texels, so they are the same numbers.
    let strength = 0.02;
    let graph = MaterialGraph::builder("test:relief")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
        .node(
            "rise",
            Math::unary(MathOp::Sin, Math::new(MathOp::Mul, "u", 3.0)),
        )
        .node(
            "fall",
            Math::unary(MathOp::Cos, Math::new(MathOp::Mul, "v", 2.0)),
        )
        .node(
            "h",
            Levels::new(Blend::new(BlendMode::Add, "rise", "fall")).in_range(-2.0, 2.0),
        )
        .node("n", NormalFromHeight::new("h").strength(strength))
        .output(
            PbrOutput::new()
                .base_color("n")
                .height("h")
                .normal_strength(strength),
        )
        .into_graph();
    let library = MaterialGraphLibrary::default();
    let params = BTreeMap::new();
    let (planes, _) = rasterise(&BakeRequest {
        graph: &graph,
        library: &library,
        params: &params,
        resolution: SMALL,
        mips: false,
        threads: threads(),
    })
    .unwrap();
    let mut compared = 0_usize;
    for (derived, through_the_node) in planes.normal.iter().zip(&planes.base_color) {
        assert!(close3(*derived, *through_the_node), "{derived:?}");
        compared += 1;
    }
    assert_eq!(compared, (SMALL as usize).pow(2));
    // And it really is a relief rather than a flat map, so the comparison is
    // about something.
    assert!(planes.normal.iter().any(|normal| normal[0].abs() > 0.01));
}

#[test]
fn a_subgraph_is_the_graph_it_instances_inlined_by_hand() {
    // The graph an instance stands for: a tinted noise with a parameter on it.
    let inner = MaterialGraph::builder("test:inner")
        .param(Param::float("wear", 0.25))
        .param(Param::color("tint", [0.8, 0.5, 0.2]))
        .node("grain", Noise::value().period(8).seed(2))
        .node(
            "worn",
            Blend::new(BlendMode::Multiply, "grain", Input::param("wear")),
        )
        .node(
            "albedo",
            Colorize::new("worn").gradient([(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])]),
        )
        .node(
            "n",
            Blend::new(BlendMode::Multiply, "albedo", Input::param("tint")),
        )
        .output(PbrOutput::new().base_color("n").roughness("worn"))
        .into_graph();
    let mut library = MaterialGraphLibrary::default();
    library.insert(inner.clone());

    // A graph that instances it, and the same graph with the instance written
    // out by hand at the values the node bound.
    let outer = MaterialGraph::builder("test:outer")
        .node(
            "inner",
            ashlar_material::nodes::Subgraph::new("test:inner")
                .param("wear", ParamValue::Float(0.75))
                .output(SurfaceOutput::BaseColor),
        )
        .node("n", Blend::new(BlendMode::Add, "inner", 0.1))
        .output(PbrOutput::new().base_color("n"))
        .into_graph();
    library.insert(outer.clone());

    let by_hand = MaterialGraph::builder("test:hand")
        .node("grain", Noise::value().period(8).seed(2))
        // The bound value, folded, and the default for the name the node did
        // not bind.
        .node("worn", Blend::new(BlendMode::Multiply, "grain", 0.75))
        .node(
            "albedo",
            Colorize::new("worn").gradient([(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])]),
        )
        .node(
            "inner",
            Blend::new(BlendMode::Multiply, "albedo", [0.8, 0.5, 0.2]),
        )
        .node("n", Blend::new(BlendMode::Add, "inner", 0.1))
        .output(PbrOutput::new().base_color("n"))
        .build()
        .unwrap();

    let instanced = lower(&library.build("test:outer").unwrap(), Target::Bake).unwrap();
    let inlined = lower(&by_hand, Target::Bake).unwrap();
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 11) / 16.0];
        let left = at(&instanced, "base_color", uv);
        let right = at(&inlined, "base_color", uv);
        assert!(close3(left, right), "at {uv:?}: {left:?} and {right:?}");
    }
    // Inlined, so it costs no more than the graph written out: the same
    // expression, folded and shared the same way.
    assert_eq!(instanced.len(), inlined.len());

    // The parameter the node bound is what reached the texels, rather than the
    // instanced graph's own default.
    let defaulted = MaterialGraph::builder("test:default")
        .node("inner", ashlar_material::nodes::Subgraph::new("test:inner"))
        .node("n", Blend::new(BlendMode::Add, "inner", 0.1))
        .output(PbrOutput::new().base_color("n"))
        .into_graph();
    library.insert(defaulted);
    let other = lower(&library.build("test:default").unwrap(), Target::Bake).unwrap();
    let uv = [0.3, 0.7];
    assert!(!close3(
        at(&instanced, "base_color", uv),
        at(&other, "base_color", uv)
    ));

    // And a node may read any output the instanced graph binds, not only the
    // colour.
    let rough = MaterialGraph::builder("test:rough")
        .node(
            "inner",
            ashlar_material::nodes::Subgraph::new("test:inner").output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph();
    library.insert(rough);
    let ir = lower(&library.build("test:rough").unwrap(), Target::Bake).unwrap();
    let value = at(&ir, "roughness", [0.3, 0.7])[0];
    let expected = at(&inlined, "base_color", [0.3, 0.7]);
    assert!(value > 0.0 && value < 1.0, "{value}");
    assert!(expected[0] > 0.0);
}

#[test]
fn a_bound_input_is_the_field_wired_into_it_and_an_unbound_one_is_its_default() {
    // A compound is written as though the field it works on were already
    // there: the input is a node, and every other node reads it the way it
    // would read a generator.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:compound")
            .node("wear", GraphInput::float("wear", 0.25))
            .node("worn", Blend::new(BlendMode::Multiply, "wear", 0.5))
            .output(PbrOutput::new().roughness("worn"))
            .into_graph(),
    );

    // Instanced over a noise, and the same expression written out by hand at
    // the field the node bound.
    let instanced = MaterialGraph::builder("test:instanced")
        .node("grain", Noise::value().period(8).seed(2))
        .node(
            "inner",
            Subgraph::new("test:compound")
                .input("wear", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    let by_hand = MaterialGraph::builder("test:hand")
        .node("grain", Noise::value().period(8).seed(2))
        .node("worn", Blend::new(BlendMode::Multiply, "grain", 0.5))
        .output(PbrOutput::new().roughness("worn"))
        .build()
        .unwrap();
    let bound = lower(&instanced, Target::Bake).unwrap();
    let hand = lower(&by_hand, Target::Bake).unwrap();
    for step in 0_u16..64 {
        let uv = [f32::from(step) / 64.0, f32::from(step % 11) / 16.0];
        let left = float_at(&bound, uv);
        let right = float_at(&hand, uv);
        assert!(close(left, right), "at {uv:?}: {left} and {right}");
    }
    // Texel for texel and instruction for instruction: the bound field is the
    // outer graph's own value, so the instance costs what writing it out costs.
    assert_eq!(bound.len(), hand.len());

    // With nothing wired in, the input is the literal it declares, and the
    // compound still builds, lowers and bakes on its own. That is what lets a
    // library graph be authored and looked at before anything instances it.
    let unbound = MaterialGraph::builder("test:unbound")
        .node(
            "inner",
            Subgraph::new("test:compound").output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    let ir = lower(&unbound, Target::Bake).unwrap();
    assert!(close(float_at(&ir, [0.3, 0.7]), 0.25 * 0.5));
    assert_eq!(unbound.period(), Period::UNIT);
}

#[test]
fn a_colour_bound_to_a_float_input_arrives_as_its_luminance() {
    // The conversion is the one the graph allows everywhere else, and it
    // happens where the value is read rather than where it was made: the
    // instance is built for a `Color` binding, and the input converts at the
    // type it declares.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:grey")
            .node("lum", GraphInput::float("lum", 0.0))
            .output(PbrOutput::new().roughness("lum"))
            .into_graph(),
    );
    let material = MaterialGraph::builder("test:tinted")
        .node("grain", Noise::value().period(8).seed(2))
        .node("rgb", Combine::new("grain", 0.0, 0.0))
        .node(
            "inner",
            Subgraph::new("test:grey")
                .input("lum", "rgb")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().base_color("rgb").roughness("inner"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    // The node's own port is the type the input declares, whatever was bound.
    assert_eq!(
        material.port("inner").map(|port| port.value_type),
        Some(ValueType::Float)
    );
    let ir = lower(&material, Target::Bake).unwrap();
    for uv in [[0.1, 0.2], [0.35, 0.6], [0.8, 0.95]] {
        let colour = colour_at(&ir, uv);
        assert!(colour[0] > 0.0, "the source varies: {colour:?}");
        assert!(close(float_at(&ir, uv), luminance(colour)), "{colour:?}");
    }
}

#[test]
fn a_compound_that_instances_another_binds_its_own_field_and_gets_it_back() {
    // The environment a binding pushes is a stack, and a wrong answer here is
    // silent: an instance that did not pop would leave the inner graph's
    // bindings in scope, and every input of the outer one would quietly read
    // the wrong field. So `mid` binds its own input into `deep` and then reads
    // that input again afterwards, where a lost pop would show.
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:deep")
            .node("wear", GraphInput::float("wear", 0.0))
            .node("twice", Math::new(MathOp::Mul, "wear", 2.0))
            .output(PbrOutput::new().roughness("twice"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("test:mid")
            .node("field", GraphInput::float("field", 0.0))
            .node(
                "deep",
                Subgraph::new("test:deep")
                    .input("wear", "field")
                    .output(SurfaceOutput::Roughness),
            )
            .node("sum", Math::new(MathOp::Add, "deep", "field"))
            .output(PbrOutput::new().roughness("sum"))
            .into_graph(),
    );
    let material = MaterialGraph::builder("test:outer")
        .node("grain", Noise::value().period(8).seed(5))
        .node(
            "inner",
            Subgraph::new("test:mid")
                .input("field", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().base_color("grain").roughness("inner"))
        .into_graph()
        .build_in(&library)
        .unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    for uv in [[0.1, 0.2], [0.45, 0.6], [0.9, 0.05]] {
        // Three times the field: twice through the inner instance and once
        // more in the graph that bound it.
        let grain = colour_at(&ir, uv)[0];
        assert!(close(float_at(&ir, uv), 3.0 * grain), "at {uv:?}: {grain}");
    }
}

/// Periods for the properties below: any integer count a generator takes, of
/// which four at once still have a multiple a bake can carry.
const PERIODS: [u32; 8] = [1, 2, 3, 4, 5, 7, 8, 12];

/// The margin a repeat is checked at.
///
/// Five decimals rather than the six an example test uses: a coordinate a
/// whole repeat along is a different number reaching the same lattice cell, so
/// what comes back has been rounded twice. It is still a four-hundredth of the
/// step of an eight-bit channel, which is the resolution the claim is for.
///
/// The margin is load-bearing in one direction. Forming `t + 1` drops about
/// nine mantissa bits of a small `t`, and a hard-edged mask turns a rounding
/// that size into a flipped step near an edge, which is a whole channel apart
/// rather than five decimals. At the counts [`PERIODS`] carries, on the dyadic
/// texels drawn below, the rounding is far inside the margin; a wall of a
/// thousand rows would fail this comparison on arithmetic alone. So widening
/// those counts wants a coordinate grid coarse enough that `t + 1` is exact
/// rather than a wider margin, which would only hide a lowering that drifted.
fn tiles(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter()
        .zip(right)
        .all(|(l, r)| l.to_bits() == r.to_bits() || (l - r).abs() < 1e-5)
}

/// A row-to-row brick bond, drawn across the interval rather than pinned to
/// the two offsets that always close.
///
/// Whether a wall comes back to itself in v is this one field's business: the
/// bond has to reach a whole brick within the wall's rows, and a wall whose
/// bond drifts instead is refused at `nodes[n].offset` rather than baked.
/// Pinning the offset to zero or a half would leave the properties below with
/// nothing to say about the one generator in this set whose periodicity is not
/// trivial, so the simple fractions are drawn whether or not their denominator
/// divides the rows, and a free draw lands on a bond that comes back nowhere.
/// What each property then checks is the claim the crate actually makes: every
/// wall that *builds* tiles, and the rest are refused by path.
fn bond() -> impl Strategy<Value = f32> {
    prop_oneof![
        3 => (1_u16..=8).prop_flat_map(|denominator| {
            (0_u16..=denominator)
                .prop_map(move |numerator| f32::from(numerator) / f32::from(denominator))
        }),
        1 => 0.0_f32..1.0,
    ]
}

/// One node of the vocabulary, wired to the sources a generated graph declares,
/// with a name for the failure message.
///
/// The weights are the number of branches each set carries, so that every node
/// comes up about as often as every other whatever set it was filed under. The
/// sets are split for length rather than by how much of the vocabulary they
/// hold, and drawing between them evenly would make a node in a set of one
/// thirteen times as likely as a node in a set of thirteen — which the coverage
/// test below would then fail on the thin end, intermittently, on a draw rather
/// than on a change. The fourth set is weighted a little above its share for
/// the same reason: it is the newest, and its nodes are the ones a property has
/// least to say about so far.
fn subject(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    prop_oneof![
        12 => first_set(periods),
        4 => second_set(periods),
        3 => pattern_set(periods),
        13 => third_set(periods),
        6 => fourth_set(periods),
    ]
    .boxed()
}

/// The nodes phase one lowered.
fn first_set(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let count = || prop::sample::select(periods);
    prop_oneof![
        // A bare coordinate does not meet itself at the seam and does not
        // claim to; through a turn of sine it does, which is what `Uv`'s own
        // documentation tells an author who wants one.
        Just(("Uv through Sin", Node::from(Math::unary(MathOp::Sin, "u")),)),
        (count(), 1_u32..3, 0.05_f32..0.95).prop_map(|(period, octaves, persistence)| (
            "Noise",
            Node::from(
                Noise::value()
                    .period(period)
                    .octaves(octaves)
                    .lacunarity(2)
                    .persistence(persistence)
            ),
        )),
        (count(), count(), bond(), 0.05_f32..0.45).prop_map(|(rows, columns, offset, mortar)| (
            "Bricks",
            Node::from(
                Bricks::new()
                    .rows(rows)
                    .columns(columns)
                    .offset(offset)
                    .mortar(mortar),
            ),
        )),
        (0.0_f32..0.4, 0.2_f32..0.6, 0.3_f32..3.0, any::<bool>()).prop_map(
            |(low, span, gamma, luminance)| {
                let mut node = Levels::new("a").in_range(low, low + span).gamma(gamma);
                if luminance {
                    node = node.luminance();
                }
                ("Levels", Node::from(node))
            }
        ),
        prop::sample::select(vec![
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Difference,
            BlendMode::Lighten,
            BlendMode::Darken,
            BlendMode::SoftLight,
            BlendMode::Dissolve,
        ])
        .prop_map(|mode| ("Blend", Node::from(Blend::new(mode, "a", "b").opacity("c")))),
        (0.1_f32..0.9, 0.0_f32..1.0).prop_map(|(middle, shade)| (
            "Colorize",
            Node::from(Colorize::new("a").gradient([
                (0.0, [0.0, shade, 1.0]),
                (middle, [shade, 1.0, 0.0]),
                (1.0, [1.0, 0.0, shade]),
            ])),
        )),
        prop::sample::select(vec![
            MathOp::Add,
            MathOp::Sub,
            MathOp::Mul,
            MathOp::Div,
            MathOp::Min,
            MathOp::Max,
            MathOp::Pow,
            MathOp::Abs,
            MathOp::Sqrt,
            MathOp::Floor,
            MathOp::Fract,
            MathOp::Step,
            MathOp::Smoothstep,
            MathOp::Sin,
            MathOp::Cos,
            MathOp::Atan2,
            MathOp::Log2,
            MathOp::Exp2,
        ])
        .prop_map(|op| ("Math", Node::from(Math::new(op, "a", "b")))),
        Just(("Invert", Node::from(Invert::new("a")))),
        Just(("Mix", Node::from(Mix::new("a", "b", "c")))),
        Just(("Combine", Node::from(Combine::new("a", "b", "c")))),
        prop::sample::select(vec![Channel::R, Channel::G, Channel::B]).prop_map(|channel| (
            "Decompose",
            Node::from(Decompose::new(
                Colorize::new("a").gradient([(0.0, [0.0, 0.5, 1.0]), (1.0, [1.0, 0.5, 0.0])]),
                channel,
            )),
        )),
    ]
    .boxed()
}

/// The second node set, which is what phase two's fifth step added.
///
/// Written apart from the first only because one function that draws every
/// node in the vocabulary is longer than anything should be; the two are one
/// strategy as far as [`case`] is concerned.
fn second_set(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let count = || prop::sample::select(periods);
    prop_oneof![
        (count(), 1_u32..3, 0.05_f32..0.95).prop_map(|(period, octaves, persistence)| (
            "Perlin",
            Node::from(
                Noise::perlin()
                    .period(period)
                    .octaves(octaves)
                    .lacunarity(2)
                    .persistence(persistence)
            ),
        )),
        (
            count(),
            count(),
            0.0_f32..=1.0,
            prop::sample::select(vec![
                VoronoiMetric::Euclidean,
                VoronoiMetric::Manhattan,
                VoronoiMetric::Chebyshev,
            ]),
            prop::sample::select(vec![
                VoronoiOutput::Distance,
                VoronoiOutput::Cell,
                VoronoiOutput::Edge,
                VoronoiOutput::Border,
            ]),
        )
            .prop_map(|(u, v, jitter, metric, output)| (
                "Voronoi",
                Node::from(
                    Voronoi::new()
                        .periods(u, v)
                        .jitter(jitter)
                        .metric(metric)
                        .output(output)
                        .width(0.1)
                ),
            )),
        (count(), count(), bond(), 0.05_f32..0.45, brick_output()).prop_map(
            |(rows, columns, offset, mortar, output)| (
                "Bricks",
                Node::from(
                    Bricks::new()
                        .rows(rows)
                        .columns(columns)
                        .offset(offset)
                        .mortar(mortar)
                        .bevel(0.2)
                        .output(output),
                ),
            )
        ),
        (
            prop::sample::select(vec![
                TilePattern::Grid,
                TilePattern::Hex,
                TilePattern::Herringbone,
            ]),
            count(),
            count(),
            brick_output(),
        )
            .prop_map(|(pattern, columns, rows, output)| {
                // Each lattice takes a whole number of cells to come back to
                // itself, so the counts are drawn and then rounded up to what
                // the pattern closes over; a count it does not close over is
                // refused at the node, which is a claim `tests/graphs.rs`
                // makes and not one for a periodicity property.
                let cells = match pattern {
                    TilePattern::Grid => [1, 1],
                    TilePattern::Hex => [1, 2],
                    TilePattern::Herringbone => [4, 4],
                };
                (
                    "Tiles",
                    Node::from(
                        Tiles::new()
                            .pattern(pattern)
                            .columns(columns * cells[0])
                            .rows(rows * cells[1])
                            .gap(0.05 / f32::from(u8::try_from(columns.max(rows)).unwrap_or(1)))
                            .bevel(0.1)
                            .output(output),
                    ),
                )
            }),
    ]
    .boxed()
}

/// The generators the second node set added that are not a noise or a lattice:
/// the patterns, the shapes and the scatter. Split off for length, as the rest
/// are.
fn pattern_set(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let count = || prop::sample::select(periods);
    prop_oneof![
        (
            prop::sample::select(vec![
                PatternKind::Stripes,
                PatternKind::Checker,
                PatternKind::Sine,
                PatternKind::Triangle,
                PatternKind::Square,
            ]),
            count(),
            count(),
            prop::sample::select(vec![
                PatternMix::Multiply,
                PatternMix::Add,
                PatternMix::Max,
                PatternMix::Min,
                PatternMix::Average,
                PatternMix::Difference,
            ]),
        )
            .prop_map(|(kind, x, y, mix)| {
                // A checker is the difference of its axes and nothing else,
                // which its own check enforces.
                let mix = if kind == PatternKind::Checker {
                    PatternMix::Difference
                } else {
                    mix
                };
                ("Pattern", Node::from(Pattern::new(kind).x(x).y(y).mix(mix)))
            }),
        (
            prop::sample::select(vec![
                ShapeKind::Circle,
                ShapeKind::Box,
                ShapeKind::Polygon,
                ShapeKind::Star,
                ShapeKind::Capsule,
                ShapeKind::Gear,
            ]),
            0.1_f32..0.4,
            0.0_f32..0.1,
            3_u32..9,
            0.0_f32..0.1,
            0.0_f32..0.2,
            0.0_f32..0.5,
            0.0_f32..0.1,
            prop::sample::select(vec![ShapeOutput::Mask, ShapeOutput::Distance]),
        )
            .prop_map(
                |(kind, size, edge, sides, round, hollow, length, depth, output)| {
                    // A capsule is the one kind whose reach is not `size`, so
                    // its length is cut to what the seam leaves rather than
                    // drawn and refused: a case that never builds says nothing
                    // about tiling. A whisker short of what is left, because
                    // the check adds half the length, the size and the edge
                    // back in a different order than this subtracts them, and a
                    // length cut to exactly the room comes back an ulp past a
                    // half often enough to fail a run.
                    let length = length.min((0.5 - size - edge).max(0.0) * 2.0 * 0.99);
                    (
                        "Shape",
                        Node::from(
                            Shape::new(kind)
                                .size(size)
                                .edge(edge)
                                .sides(sides)
                                .round(round)
                                .hollow(hollow)
                                .length(length)
                                .depth(depth)
                                .output(output),
                        ),
                    )
                },
            ),
        (
            prop::sample::select(vec![1_u32, 4, 9, 16]),
            0.02_f32..0.2,
            0.002_f32..0.02,
            0.0_f32..360.0,
        )
            .prop_map(|(number, length, width, angle)| (
                "Scratches",
                Node::from(
                    Scratches::new()
                        .count(number)
                        .length(length)
                        .width(width)
                        .angle(angle)
                        .angle_spread(180.0),
                ),
            )),
    ]
    .boxed()
}

/// The resamplers and the rest of the pointwise filters, split off for the
/// same reason [`second_set`] is: length.
fn third_set(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let count = || prop::sample::select(periods);
    prop_oneof![
        (
            1_u32..4,
            1_u32..4,
            prop::sample::select(vec![0.0_f32, 90.0, 180.0, 270.0]),
            -1.0_f32..1.0,
        )
            .prop_map(|(u, v, rotate, translate)| (
                "Transform",
                Node::from(
                    Transform::new("a")
                        .scales(
                            f32::from(u8::try_from(u).unwrap_or(1)),
                            f32::from(u8::try_from(v).unwrap_or(1))
                        )
                        .rotate(rotate)
                        .translate(translate, translate * 0.5),
                ),
            )),
        (count(), count(), 0.0_f32..0.5, 0.0_f32..1.0).prop_map(|(u, v, overlap, variation)| (
            "Tile",
            Node::from(
                Tile::new("a")
                    .counts(u, v)
                    .overlap(overlap)
                    .scale_variation(variation)
                    .rotation_variation(variation)
                    .opacity_variation(variation),
            ),
        )),
        (0.0_f32..0.5).prop_map(|amount| ("Warp", Node::from(Warp::new("a", "b").amount(amount)))),
        prop::sample::select(vec![MirrorAxis::U, MirrorAxis::V])
            .prop_map(|axis| ("Mirror", Node::from(Mirror::new("a").axis(axis)))),
        // Only the quadrant fold tiles; the angular one is free by design and
        // is checked by hand instead.
        Just(("Kaleidoscope", Node::from(Kaleidoscope::new("a").count(4)),)),
        (0.2_f32..0.8, 0.1_f32..0.9).prop_map(|(x, y)| (
            "Curve",
            Node::from(Curve::new("a").points([[0.0, 0.0], [x, y], [1.0, 1.0]])),
        )),
        (-0.2_f32..0.2, 0.5_f32..2.0, 0.0_f32..360.0, 0.0_f32..2.0).prop_map(
            |(brightness, contrast, hue, saturation)| (
                "Adjust",
                Node::from(
                    Adjust::new("a")
                        .brightness(brightness)
                        .contrast(contrast)
                        .hue(hue)
                        .saturation(saturation),
                ),
            )
        ),
        Just(("Clamp", Node::from(Clamp::new("a").range("b", "c")))),
        Just(("Switch", Node::from(Switch::new("a", "b", "c")))),
        (0.0_f32..0.5).prop_map(|amount| (
            "DirectionalWarp",
            Node::from(DirectionalWarp::new("a", "b").amount(amount)),
        )),
        (0.0_f32..0.6, 0.0_f32..0.4, 0.0_f32..0.3).prop_map(|(low, span, softness)| (
            "HeightToMask",
            Node::from(HeightToMask::band("a", low, low + span).softness(softness)),
        )),
        // A scatter whose instances are gated by a field: the mask is the one
        // input of a tile that is read across the whole repeat rather than
        // inside a cell, so it is the one that can stop it tiling.
        (count(), count(), 0.0_f32..1.0).prop_map(|(u, v, opacity)| (
            "a masked Tile",
            Node::from(
                Tile::new("a")
                    .counts(u, v)
                    .mask("b")
                    .opacity_variation(opacity),
            ),
        )),
    ]
    .boxed()
}

/// The nodes this phase added: the two that lay their field inside the repeat
/// rather than across it, the two that read a signed distance, and the cloth.
/// Split off from [`third_set`] for the same reason that one is: length.
fn fourth_set(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let count = || prop::sample::select(periods);
    prop_oneof![
        // The window is drawn well inside the repeat and the hole well inside
        // the window, because a disc that reached the seam is refused at
        // `radius` rather than tiled, and the claim here is about the ones that
        // build. The twist is drawn either way round: it is an angle carried
        // per unit of radius, and a negative one winds the other way.
        (
            0.25_f32..0.5,
            0.0_f32..0.2,
            count(),
            count(),
            -2.0_f32..2.0,
            0.0_f32..1.0,
        )
            .prop_map(|(radius, inner, turns, rings, twist, outside)| (
                "CircleMap",
                Node::from(
                    CircleMap::new("a")
                        .radius(radius)
                        .inner(inner)
                        .turns(turns)
                        .rings(rings)
                        .twist(twist)
                        .outside(outside),
                ),
            )),
        // A ring of instances drawn well inside the repeat: the radius and the
        // instance width together have to leave the corner of an instance of
        // room before the seam, or the node is refused at `radius` rather than
        // tiled, and the claim here is about the ones that build. The widths
        // are drawn against that corner rather than against half a width,
        // because half of these cases turn their instances and a case that is
        // refused says nothing: the widest ring here reaches `0.3 + 0.12 * 2 /
        // sqrt(2)`, which is under a half. Every variation is then drawn
        // across its whole range — the three that change an instance's size or
        // place only ever take away, and the one that turns it is already paid
        // for in the width the radius was chosen against.
        (
            0.15_f32..0.3,
            0.0_f32..0.1,
            0.05_f32..0.12,
            count(),
            1_u32..3,
            prop::array::uniform4(0.0_f32..1.0),
            any::<bool>(),
        )
            .prop_map(|(radius, inner, scale, slots, rings, variation, facing)| {
                let mut node = CircleSplatter::new("a")
                    .mask("b")
                    .count(slots)
                    .rings(rings)
                    .radius(radius)
                    .inner(inner)
                    .scale(scale)
                    .scale_variation(variation[0])
                    .rotation_variation(variation[1])
                    .radius_variation(variation[2])
                    .opacity_variation(variation[3]);
                if facing {
                    node = node.face_centre();
                }
                ("CircleSplatter", Node::from(node))
            },),
        // A boolean is pointwise arithmetic, so what it has to say here is
        // that it carries its two sources' seams rather than laying one of its
        // own: the fields drawn into it are noises rather than distances,
        // because `min`, `max` and the fillet between them do not care what
        // the numbers mean. The fillet is drawn at zero as well, which is the
        // one width where the node emits the hard boolean instead.
        (
            prop::sample::select(vec![SdfOp::Union, SdfOp::Intersect, SdfOp::Subtract]),
            prop_oneof![1 => Just(0.0_f32), 3 => 0.01_f32..0.5],
        )
            .prop_map(|(op, smooth)| (
                "SdfCombine",
                Node::from(SdfCombine::new(op, "a", "b").smooth(smooth)),
            )),
        (prop_oneof![1 => Just(0.0_f32), 3 => 0.01_f32..0.5])
            .prop_map(|edge| ("SdfMask", Node::from(SdfMask::new("a").edge(edge)),)),
        // The thread counts are drawn as multiples of whatever repeat the
        // pattern came out at, because a count the crossing does not divide is
        // refused at `x` rather than tiled and a case that never builds says
        // nothing. That is the one field of this node a property has anything
        // to prove about: the crossing has to close over the seam, and it is
        // the count rather than the geometry that decides whether it does.
        (
            prop_oneof![
                2 => Just(WeavePattern::Plain),
                2 => (3_u32..7).prop_map(|step| WeavePattern::Twill { step }),
                1 => prop::sample::select(vec![5_u32, 7, 8])
                    .prop_map(|step| WeavePattern::Satin { step }),
            ],
            1_u32..4,
            1_u32..4,
            0.2_f32..0.95,
            prop::sample::select(vec![
                WeaveOutput::Mask,
                WeaveOutput::Height,
                WeaveOutput::Warp,
                WeaveOutput::Weft,
                WeaveOutput::Id,
            ]),
            0_u32..4,
        )
            .prop_map(|(pattern, across, up, width, output, seed)| {
                let repeat = pattern.repeat();
                (
                    "Weave",
                    Node::from(
                        Weave::new()
                            .x(repeat * across)
                            .y(repeat * up)
                            .width(width)
                            .pattern(pattern)
                            .output(output)
                            .seed(seed),
                    ),
                )
            }),
    ]
    .boxed()
}

/// One output of a wall or a tiling, drawn.
fn brick_output() -> impl Strategy<Value = BrickOutput> {
    prop::sample::select(vec![
        BrickOutput::Mask,
        BrickOutput::Bevel,
        BrickOutput::Id,
        BrickOutput::Fill,
    ])
}

/// A texel coordinate inside the unit, drawn at random on a dyadic grid.
///
/// Dyadic on purpose. A whole repeat along is `t + 1`, and the claim being
/// checked is about the field rather than about `f32`: with a coordinate of
/// this shape every period in [`PERIODS`] scales it exactly, so the lattice
/// cell and the weights inside it are the same bits on both sides and a
/// difference is the lowering's. A coordinate off the grid would carry a
/// rounding of its own into a gamma or a smoothstep and make the margin a
/// question about arithmetic.
fn texel() -> impl Strategy<Value = f32> {
    (0_u16..256).prop_map(|step| f32::from(step) / 256.0)
}

/// The material a generated graph builds, or `None` where the crate refused
/// the graph.
///
/// Exactly one draw in [`subject`] can be refused: a brick bond that does not
/// come back to a whole brick within its rows lays a wall that does not meet
/// itself in v, and the crate rejects it at `offset` instead of baking it.
/// That refusal is the same promise the properties below check, so it is held
/// to its path here rather than skipped quietly — a graph refused anywhere
/// else fails the test, because nothing else in this set may be refused.
fn built(graph: MaterialGraph) -> Result<Option<Material>, TestCaseError> {
    match graph.build() {
        Ok(material) => Ok(Some(material)),
        Err(error) => {
            prop_assert!(
                error.path.ends_with(".offset"),
                "a generated graph was refused at {}: {}",
                error.path,
                error.reason,
            );
            Ok(None)
        }
    }
}

/// One buffered node, wired to the sources a generated graph declares.
///
/// The settings are drawn small: what is under test is that a filter wraps, and
/// a radius of half the repeat would say the same thing about the wrap while
/// costing a hundred times as much to say it. A radius under half a texel of
/// the plane is an identity filter, which is a case worth drawing too, because
/// it is the one where the sampler alone has to hold the seam up.
fn buffered_subject(periods: &'static [u32]) -> BoxedStrategy<(&'static str, Node)> {
    let _ = periods;
    let radius = || prop::sample::select(vec![0.0_f32, 0.002, 0.008, 0.03]);
    prop_oneof![
        radius().prop_map(|radius| ("Blur", Node::from(Blur::new("a").radius(radius)))),
        (radius(), 0.1_f32..1.0).prop_map(|(radius, strength)| (
            "OcclusionFromHeight",
            Node::from(
                OcclusionFromHeight::new("a")
                    .radius(radius)
                    .strength(strength)
            )
        )),
        (0.2_f32..0.8, 0.02_f32..0.5).prop_map(|(threshold, range)| (
            "Distance",
            Node::from(Distance::new("a").threshold(threshold).range(range))
        )),
        radius().prop_map(|radius| ("Erode", Node::from(Erode::new("a").radius(radius)))),
        radius().prop_map(|radius| ("Dilate", Node::from(Dilate::new("b").radius(radius)))),
        prop::sample::select(vec![16_u32, 64, 256]).prop_map(|resolution| (
            "Buffer",
            Node::from(Buffer::new("c").resolution(resolution))
        )),
        // A normal is buffered in everything but name: it differences the
        // plane its height rasterised into, at the plane's own texel size.
        (0.002_f32..0.05).prop_map(|strength| (
            "NormalFromHeight",
            Node::from(NormalFromHeight::new("a").strength(strength))
        )),
        (radius(), 0.0_f32..360.0).prop_map(|(radius, angle)| (
            "a directional Blur",
            Node::from(Blur::directional("a", angle).radius(radius))
        )),
        // The one filter that reads two planes. The source and the height are
        // different noises on purpose: a walk that read its own field would
        // tile whatever the guide did.
        (radius(), 1_u32..=8).prop_map(|(radius, steps)| (
            "a slope Blur",
            Node::from(Blur::slope("a", "b").radius(radius).steps(steps))
        )),
        (
            radius(),
            prop::sample::select(vec![CurvatureKind::Laplacian, CurvatureKind::Blurred]),
            prop::sample::select(vec![
                CurvatureOutput::Signed,
                CurvatureOutput::Peaks,
                CurvatureOutput::Cavity,
            ]),
        )
            .prop_map(|(radius, kind, output)| (
                "Curvature",
                Node::from(
                    Curvature::new("a")
                        .radius(radius)
                        .kind(kind)
                        .output(output)
                        .strength(4.0)
                )
            )),
        radius().prop_map(|radius| (
            "EdgeDetect",
            Node::from(EdgeDetect::new("b").radius(radius))
        )),
    ]
    .boxed()
}

/// A graph whose node `n` is the subject, over three noises it may read.
fn case(
    periods: &'static [u32],
    subject: fn(&'static [u32]) -> BoxedStrategy<(&'static str, Node)>,
) -> impl Strategy<Value = (&'static str, MaterialGraph)> {
    let count = || prop::sample::select(periods);
    (count(), count(), count(), count(), subject(periods)).prop_map(
        |(a, bu, bv, c, (label, node))| {
            let graph = MaterialGraph::builder("test:subject")
                .node("uv", Uv::new())
                .node("u", Decompose::new("uv", Channel::R))
                .node("v", Decompose::new("uv", Channel::G))
                .node("a", Noise::value().period(a))
                // One source tiles differently in the two axes, so a rule
                // that is right in u and wrong in v has somewhere to show.
                .node("b", Noise::value().periods(bu, bv))
                .node("c", Noise::value().period(c))
                .node("n", node)
                .output(PbrOutput::new().base_color("n"))
                .into_graph();
            (label, graph)
        },
    )
}

proptest! {
    // More cases than phase one ran, because the vocabulary this draws from is
    // three times the size: a count that gave every node a dozen draws then
    // gives it four now.
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Every node in this set meets itself at both seams: the texel at `u = 0`
    /// and the one at `u = 1` are the same texel of the next repeat along, and
    /// a bake that wrapped would show the difference as a line.
    #[test]
    fn every_node_meets_itself_at_both_seams(
        (label, graph) in case(&PERIODS, subject),
        drawn in texel(),
    ) {
        if let Some(material) = built(graph)? {
            meets_itself(label, &Field::of(&material), drawn)?;
        }
    }

    /// And it repeats across the plane rather than only meeting at the seam:
    /// a texel and the same texel a whole repeat along carry the same value.
    ///
    /// One repeat is the UV unit, which is what a `Tiled` period promises and
    /// what a bake writes. It is *not* `1 / period`: that count is the lattice
    /// a field is built on rather than a shift the field is invariant under,
    /// and the eight cells of a noise of period eight hash differently on
    /// purpose. Where the cells really are copies of one another the finer
    /// invariance holds too, and brickwork's bond is checked for it by hand
    /// above; nothing in the vocabulary is allowed to depend on it.
    #[test]
    fn every_node_repeats_a_whole_uv_unit_along(
        (label, graph) in case(&PERIODS, subject),
        drawn in texel(),
        across in texel(),
    ) {
        if let Some(material) = built(graph)? {
            prop_assert!(material.port("n").expect("the subject has a port").period.is_tiled());
            repeats(label, &Field::of(&material), drawn, across)?;
        }
    }
}

proptest! {
    // Fewer cases than the pointwise set above, because each of these
    // rasterises a plane and runs a filter over it: a jump flood at 256 is a
    // hundred times the work of evaluating an expression at a dozen texels.
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// The same two claims over the buffered nodes, through the planes a bake
    /// at a small resolution leaves behind.
    ///
    /// What this reaches that the set above does not is the other half of a
    /// buffered node: a plane is one repeat of a field, and the field it stands
    /// for is that plane read with wrap. So this is the claim that the period
    /// inference puts on these nodes — they keep what reached them — held
    /// against what the expression actually answers, at the seam and a repeat
    /// along, for a plane of every size a `Buffer` can pin.
    ///
    /// It is deliberately not the claim that the *filters* wrap: a filter that
    /// read a pad instead of the far edge would leave a plane that still met
    /// itself when sampled, and only the picture would be wrong. That is pinned
    /// by hand, filter by filter, in `tests/buffers.rs`, against fields whose
    /// answer is different if the neighbourhood stopped at the edge.
    #[test]
    fn every_buffered_node_tiles_through_the_plane_it_rasterised(
        (label, graph) in case(&PERIODS, buffered_subject),
        drawn in texel(),
        across in texel(),
    ) {
        if let Some(material) = built(graph)? {
            prop_assert!(material.port("n").expect("the subject has a port").period.is_tiled());
            let field = Field::of(&material);
            meets_itself(label, &field, drawn)?;
            repeats(label, &field, drawn, across)?;
        }
    }
}

/// Not a property: a count of how often the generated case reaches each node,
/// so neither property above can quietly stop covering one of them.
///
/// The brick bond is counted twice over, by whether the wall it lays builds.
/// A property that only ever saw refused walls would pass while saying nothing
/// about brickwork, and one that only ever saw closing bonds is the state this
/// test was added to leave behind, so both answers have to keep arriving.
#[test]
fn the_subject_strategy_reaches_every_node_in_the_set() {
    use proptest::{
        strategy::ValueTree,
        test_runner::{Config, TestRunner},
    };
    let mut runner = TestRunner::new(Config::default());
    let mut seen: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for _ in 0..1024 {
        for subject in [subject, buffered_subject] {
            let (label, _) = case(&PERIODS, subject)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            *seen.entry(label).or_default() += 1;
        }
        let (label, graph) = case(&PERIODS, subject)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        if label == "Bricks" {
            let closes = graph.build().is_ok();
            *seen
                .entry(if closes {
                    "a wall that tiles"
                } else {
                    "a wall that is refused"
                })
                .or_default() += 1;
        }
    }
    for label in [
        "Uv through Sin",
        "Noise",
        "Bricks",
        "Levels",
        "Blend",
        "Colorize",
        "Math",
        "Invert",
        "Mix",
        "Combine",
        "Decompose",
        "a wall that tiles",
        "a wall that is refused",
        "Perlin",
        "Voronoi",
        "Tiles",
        "Pattern",
        "Shape",
        "Scratches",
        "Transform",
        "Tile",
        "Warp",
        "Mirror",
        "Kaleidoscope",
        "Curve",
        "Adjust",
        "Clamp",
        "Switch",
        "Blur",
        "OcclusionFromHeight",
        "Distance",
        "Erode",
        "Dilate",
        "NormalFromHeight",
        "Buffer",
        "DirectionalWarp",
        "HeightToMask",
        "a masked Tile",
        "a directional Blur",
        "a slope Blur",
        "Curvature",
        "EdgeDetect",
        "CircleMap",
        "CircleSplatter",
        "SdfCombine",
        "SdfMask",
        "Weave",
    ] {
        assert!(
            seen.get(label).copied().unwrap_or_default() >= 8,
            "{label} came up {:?} times in 1024",
            seen.get(label),
        );
    }
}

// --------------------------------------------------------------------------
// The world-space nodes
// --------------------------------------------------------------------------

/// A material lowered for a shader with nothing live, which is the only target
/// a world-space input has an answer for.
///
/// The bake is the reference everywhere else in this file; here it is the thing
/// being refused, so the fixtures below say `Shader` and the refusal has a test
/// of its own.
fn shader_node(node: impl Into<Node>) -> Ir {
    let material = MaterialGraph::builder("test:world")
        .node("uv", Uv::new())
        .node("u", Decompose::new("uv", Channel::R))
        .node("v", Decompose::new("uv", Channel::G))
        .node("n", node)
        .output(PbrOutput::new().roughness("n"))
        .build()
        .unwrap();
    lower(&material, Target::Shader { live: Vec::new() }).unwrap()
}

/// The roughness at one texel, at a world position and a world normal.
fn world_at(ir: &Ir, uv: [f32; 2], world_pos: [f32; 3], world_normal: [f32; 3]) -> f32 {
    let root = ir.root("roughness").expect("a bound output");
    let inputs = Inputs {
        world_pos,
        world_normal,
        ..Inputs::default()
    };
    Interpreter::new(ir).eval(uv, &inputs, root).unwrap()[0]
}

#[test]
fn a_world_space_input_is_refused_by_a_bake_and_answers_for_a_shader() {
    // The rule this item added, and the one thing in the vocabulary a *target*
    // refuses rather than a backend: a texture has no axis to store a world
    // position along, and folding one to zero would bake one point of the
    // surface across the whole map without saying so. So it is a refusal by
    // node path, and the message names the surface that does answer.
    let refused = |node: Node| {
        let material = MaterialGraph::builder("test:world")
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
            .build()
            .unwrap();
        let error = lower(&material, Target::Bake).unwrap_err();
        assert_eq!(error.path, "nodes[n]");
        assert!(
            error.reason.contains("Shader surface"),
            "a refusal that does not say what to do instead: {error}"
        );
        // And the same graph lowers for the target that has the value.
        lower(&material, Target::Shader { live: Vec::new() }).expect("a shader answers");
        error.reason
    };
    assert!(refused(WorldPos::new().into()).contains("a world position"));
    assert!(refused(WorldNormal::new().into()).contains("a world normal"));
    // The two nodes built out of them refuse in the same words at their own
    // path, because they emit the same instruction: an author who wrote a
    // triplanar is told about the triplanar rather than about a node they
    // never typed.
    assert!(refused(WorldMask::up().into()).contains("a world normal"));
    assert!(
        refused(WorldMask::below(2.0).into()).contains("a world position"),
        "a height mask reads the position rather than the normal"
    );
    assert!(refused(Triplanar::new(0.5).into()).contains("a world position"));

    // And it is a refusal about the graph rather than about its outputs: the
    // driver lowers every node a graph declares, so one nothing reads refuses
    // the bake too, and names itself rather than leaving an author to find it.
    let unread = MaterialGraph::builder("test:unread")
        .node("grain", Noise::value().period(8))
        .node("spare", WorldMask::up())
        .output(PbrOutput::new().roughness("grain"))
        .build()
        .unwrap();
    assert_eq!(
        lower(&unread, Target::Bake).unwrap_err().path,
        "nodes[spare]"
    );

    // The other two runtime inputs are unchanged: zero is a picture for them —
    // the instant the app started, on a face nobody cut — so they still bake.
    for node in [Node::from(Time::new()), Node::from(CutFlag::new())] {
        let material = MaterialGraph::builder("test:clock")
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
            .build()
            .unwrap();
        let ir = lower(&material, Target::Bake).expect("a clock bakes at zero");
        assert!(close(
            Interpreter::new(&ir)
                .eval(
                    [0.5, 0.5],
                    &Inputs::default(),
                    ir.root("roughness").unwrap()
                )
                .unwrap()[0],
            0.0
        ));
    }
}

#[test]
fn a_world_mask_is_one_where_its_axis_points_and_ramps_across_its_own_bound() {
    let up = shader_node(WorldMask::up());
    let anywhere = [0.0, 0.0, 0.0];
    // Straight up is one, straight along is zero, and the default threshold is
    // the cosine of sixty degrees: a face leaning that far back is a half.
    assert!(close(
        world_at(&up, [0.5, 0.5], anywhere, [0.0, 1.0, 0.0]),
        1.0
    ));
    assert!(close(
        world_at(&up, [0.5, 0.5], anywhere, [1.0, 0.0, 0.0]),
        0.0
    ));
    assert!(close(
        world_at(&up, [0.5, 0.5], anywhere, [0.0, -1.0, 0.0]),
        0.0
    ));
    assert!(close(
        world_at(&up, [0.5, 0.5], anywhere, [0.0, 0.5, 0.0]),
        0.5
    ));
    // The mask is a fact about the fragment and not about the texel: the same
    // normal answers the same number wherever in the repeat it is read.
    for uv in [[0.0_f32, 0.0_f32], [0.125, 0.875], [0.5, 0.5]] {
        assert!(close(world_at(&up, uv, anywhere, [0.0, 1.0, 0.0]), 1.0));
    }

    // A softness of zero is a hard edge, which rests on the crate's own
    // guarded smoothstep answering a step where its edges meet.
    let hard = shader_node(WorldMask::up().softness(0.0));
    assert!(close(
        world_at(&hard, [0.5, 0.5], anywhere, [0.0, 0.499, 0.0]),
        0.0
    ));
    assert!(close(
        world_at(&hard, [0.5, 0.5], anywhere, [0.0, 0.501, 0.0]),
        1.0
    ));

    // A height mask reads the position instead, in metres, and `below` is the
    // same ramp on the downward axis rather than a second rule.
    let low = shader_node(WorldMask::below(2.0).softness(0.5));
    let flat = [0.0, 1.0, 0.0];
    assert!(close(
        world_at(&low, [0.5, 0.5], [3.0, 0.0, -7.0], flat),
        1.0
    ));
    assert!(close(
        world_at(&low, [0.5, 0.5], [3.0, 2.0, -7.0], flat),
        0.5
    ));
    assert!(close(
        world_at(&low, [0.5, 0.5], [3.0, 4.0, -7.0], flat),
        0.0
    ));
    let high = shader_node(WorldMask::above(2.0).softness(0.5));
    assert!(close(
        world_at(&high, [0.5, 0.5], [0.0, 4.0, 0.0], flat),
        1.0
    ));
    assert!(close(
        world_at(&high, [0.5, 0.5], [0.0, 0.0, 0.0], flat),
        0.0
    ));
}

#[test]
fn a_triplanar_reads_its_source_on_the_plane_the_face_points_along() {
    // A source that says which coordinate it was read at: `u + 10 v`, so the
    // answer names the plane and the metres in one number.
    let source = |builder: MaterialGraphBuilder| {
        builder
            .node("tenth", Math::new(MathOp::Mul, "v", 10.0))
            .node("marked", Math::new(MathOp::Add, "u", "tenth"))
    };
    let material = source(
        MaterialGraph::builder("test:triplanar")
            .node("uv", Uv::new())
            .node("u", Decompose::new("uv", Channel::R))
            .node("v", Decompose::new("uv", Channel::G)),
    )
    .node(
        "n",
        Triplanar::new("marked").tile_metres(2.0).sharpness(8.0),
    )
    .output(PbrOutput::new().roughness("n"))
    .build()
    .unwrap();
    let ir = lower(&material, Target::Shader { live: Vec::new() }).unwrap();
    let position = [1.0, 3.0, 5.0];
    let at = |normal: [f32; 3]| world_at(&ir, [0.25, 0.75], position, normal);
    // Two metres to a repeat, so the world point is (0.5, 1.5, 2.5) in the
    // source's own units. Each plane keeps the two axes it does not run along.
    let (x, y, z) = (0.5, 1.5, 2.5);
    assert!(
        close(at([1.0, 0.0, 0.0]), z + 10.0 * y),
        "the x plane is zy"
    );
    assert!(
        close(at([0.0, 1.0, 0.0]), x + 10.0 * z),
        "the y plane is xz"
    );
    assert!(
        close(at([0.0, 0.0, 1.0]), x + 10.0 * y),
        "the z plane is xy"
    );
    // A face pointing the other way along an axis reads the same plane: the
    // weights are absolute values, which is what makes the two sides of a slab
    // one surface.
    assert!(close(at([0.0, -1.0, 0.0]), at([0.0, 1.0, 0.0])));
    // The texel's own coordinate reaches none of it. A triplanar is not a field
    // over UV, which is what its unit period says.
    assert!(close(
        world_at(&ir, [0.9, 0.1], position, [0.0, 1.0, 0.0]),
        at([0.0, 1.0, 0.0])
    ));
    assert_eq!(
        material.port("n").map(|port| port.period),
        Some(Period::UNIT)
    );

    // A diagonal is the weighted mean of the two planes it lies between, at the
    // sharpness the node was given, and it lies between them: neither past the
    // pair nor equal to either.
    let diagonal = at([0.6, 0.8, 0.0]);
    let (along_x, along_y) = (at([1.0, 0.0, 0.0]), at([0.0, 1.0, 0.0]));
    let (low, high) = (along_x.min(along_y), along_x.max(along_y));
    assert!(diagonal > low && diagonal < high, "{diagonal}");
    let (wx, wy) = (0.6_f32.powf(8.0), 0.8_f32.powf(8.0));
    assert!(close(diagonal, (wx * along_x + wy * along_y) / (wx + wy)));
    // A normal of zero has no axis to favour, and the guarded division answers
    // zero rather than a NaN spread across the surface.
    assert!(close(at([0.0, 0.0, 0.0]), 0.0));
}

#[test]
fn a_triplanar_refuses_a_source_that_does_not_tile() {
    // The one check this node makes on what reached it, and the reason it makes
    // it: the world-space field repeats every `tile_metres` along each axis, so
    // a source that does not meet itself puts a seam on every one of those
    // metre lines — which is the seam a triplanar is used to remove.
    let material = MaterialGraph::builder("test:untiled")
        .node("grain", Noise::value().period(8))
        // A non-integer scale is the plainest way to write a field that does
        // not tile; inference calls it free and the output would too.
        .node("skewed", Transform::new("grain").scale(1.5))
        .node("n", Triplanar::new("skewed"))
        .output(PbrOutput::new().roughness("n"))
        .build();
    let error = material.expect_err("a source that does not tile is refused");
    assert_eq!(error.path, "nodes[n].inputs[source]");
    assert!(error.reason.contains("does not tile"), "{error}");

    // And the fields it checks on itself.
    let refused = |node: Triplanar, field: &str| {
        let error = MaterialGraph::builder("test:triplanar")
            .node("n", node)
            .output(PbrOutput::new().roughness("n"))
            .build()
            .expect_err("refused");
        assert_eq!(error.path, format!("nodes[n].{field}"));
    };
    refused(Triplanar::new(0.5).tile_metres(0.0), "tile_metres");
    refused(Triplanar::new(0.5).sharpness(0.5), "sharpness");
}

#[test]
fn gradient_warp_uses_guide_derivatives_and_intensity_warp_uses_distance() {
    // A linear guide has a known derivative, away from its wrap discontinuity.
    let gradient = wired(|b| {
        b.node(
            "guide",
            Math::new(MathOp::Add, Math::new(MathOp::Mul, "u", 2.0), "v"),
        )
        .node("n", GradientWarp::new("u", "guide").amount(0.05))
        .output(PbrOutput::new().roughness("n"))
    });
    assert!((float_at(&gradient, [0.3, 0.4]) - 0.4).abs() < 1e-5);
    let constant = wired(|b| {
        b.node("n", GradientWarp::new("v", 0.8).amount(5.0))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&constant, [0.3, 0.4]), 0.4));
    let intensity = wired(|b| {
        b.node("n", IntensityWarp::new("v", "u").angle(0.25).amount(0.5))
            .output(PbrOutput::new().roughness("n"))
    });
    assert!(close(float_at(&intensity, [0.0, 0.4]), 0.4));
    assert!(close(float_at(&intensity, [0.4, 0.4]), 0.6));
    assert!(close(float_at(&intensity, [0.8, 0.8]), 0.2));
}

#[test]
fn gradient_and_intensity_warps_round_trip_and_wrap_both_seams() {
    for node in [
        Node::from(GradientWarp::new("source", "guide").amount(0.002)),
        Node::from(
            IntensityWarp::new("source", "guide")
                .angle(0.17)
                .amount(0.1),
        ),
    ] {
        let encoded = ron::to_string(&node).unwrap();
        assert_eq!(node, ron::from_str::<Node>(&encoded).unwrap());
        let ir = wired(|b| {
            b.node("source", Noise::perlin().period(8))
                .node("guide", Noise::perlin().period(4).seed(2))
                .node("n", node)
                .output(PbrOutput::new().roughness("n"))
        });
        for t in [0.13, 0.37, 0.78] {
            assert!((float_at(&ir, [0.0, t]) - float_at(&ir, [1.0, t])).abs() < 1e-5);
            assert!((float_at(&ir, [t, 0.0]) - float_at(&ir, [t, 1.0])).abs() < 1e-5);
        }
    }
    for distance in [0.0, -0.1, f32::NAN, f32::INFINITY, 0.5] {
        assert!(
            MaterialGraph::builder("bad:warp")
                .node(
                    "warp",
                    GradientWarp::new(0.5, 0.5).sample_distance(distance)
                )
                .output(PbrOutput::new().roughness("warp"))
                .build()
                .is_err()
        );
    }
}
