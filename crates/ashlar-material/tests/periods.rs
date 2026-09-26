//! One test per periodicity rule.
//!
//! Periods are the static half of the promise that a bake tiles, so each rule
//! is pinned here on its own: a rule that quietly turned into "free" would
//! still pass a test that only asked whether the graph validates.
use ashlar_material::{
    BlendMode, GraphError, Input, MAX_PERIOD, Material, MaterialGraph, MaterialGraphLibrary,
    MathOp, Node, Param, PbrOutput, Period, SdfOp, SurfaceOutput,
    interp::{Inputs, Interpreter},
    ir::{Target, lower},
    nodes::{
        Blend, Blur, Bricks, Buffer, CircleMap, CircleSplatter, Dilate, Distance, Erode,
        GraphInput, Kaleidoscope, Levels, Math, Mirror, MirrorAxis, Noise, NormalFromHeight,
        OcclusionFromHeight, Pattern, PatternKind, Scratches, SdfCombine, SdfMask, Shape,
        ShapeKind, ShapeOutput, Subgraph, Tile, TilePattern, Tiles, Transform, Uv, Voronoi, Warp,
        Weave, WeaveOutput, WeavePattern,
    },
};

/// Build a graph of the nodes under test, with the output bound to something
/// that tiles, so a free field is a fact to read rather than an error.
fn probe(nodes: Vec<(&str, Node)>) -> Material {
    let mut builder = MaterialGraph::builder("test:period")
        .param(Param::float("wear", 0.5))
        .node("anchor", Noise::value());
    for (id, node) in nodes {
        builder = builder.node(id, node);
    }
    builder
        .output(PbrOutput::new().base_color("anchor"))
        .build()
        .expect("a valid graph")
}

/// The inferred period of one node of a graph of nodes.
fn period(nodes: Vec<(&str, Node)>, id: &str) -> Period {
    probe(nodes).port(id).expect("an inferred port").period
}

/// The period of one node that stands on its own.
fn alone(node: impl Into<Node>) -> Period {
    period(vec![("probe", node.into())], "probe")
}

#[test]
fn a_uv_a_constant_and_a_parameter_all_tile_once() {
    assert_eq!(alone(Uv::new()), Period::UNIT);
    let material = probe(vec![
        ("literal", Levels::new(0.25).into()),
        ("parameter", Levels::new(Input::param("wear")).into()),
    ]);
    for id in ["literal", "parameter"] {
        assert_eq!(material.port(id).expect("inferred").period, Period::UNIT);
    }
    // Which is what makes a constant free to blend with: it is the neutral
    // element of the least common multiple.
    assert_eq!(Period::UNIT.lcm(Period::square(12)), Period::square(12));
}

/// The numeric seam check the showcase used to make on its own pixel loop,
/// at the periods the study's two surfaces are authored at.
///
/// This is where the retired pixel loop's seam test went when the study
/// became a graph. `tests/nodes.rs` proves the rule over
/// the whole vocabulary, but only at the small counts a property test can draw
/// and still compare exactly; the study's own 4, 8, 32 and 128 are pinned here,
/// evaluated rather than inferred. The comparison is exact rather than within a
/// margin because that is the claim: a lattice coordinate a whole repeat along
/// reduces to the same cell with the same weights, so the two answers are the
/// same bits and not merely close.
#[test]
fn a_value_noise_meets_itself_at_both_seams_at_the_studys_own_periods() {
    for period in [4, 8, 32, 128] {
        let material = MaterialGraph::builder("test:seam")
            .node("noise", Noise::value().period(period))
            .output(PbrOutput::new().roughness("noise"))
            .build()
            .expect("a valid graph");
        assert_eq!(material.period(), Period::square(period));
        let ir = lower(&material, Target::Bake).expect("a lowering");
        let root = ir.root("roughness").expect("a bound output");
        let at = |uv: [f32; 2]| {
            Interpreter::new(&ir)
                .eval_float(uv, &Inputs::default(), root)
                .expect("nothing here is buffered")
        };
        for along in [0.0_f32, 0.13, 0.37, 0.99] {
            // Compared by bits: the claim is that the two coordinates reduce
            // to the same cell with the same weights, not that they are close.
            let (west, east) = (at([0.0, along]), at([1.0, along]));
            assert_eq!(
                west.to_bits(),
                east.to_bits(),
                "period {period} at v = {along}"
            );
            let (south, north) = (at([along, 0.0]), at([along, 1.0]));
            assert_eq!(
                south.to_bits(),
                north.to_bits(),
                "period {period} at u = {along}"
            );
        }
    }
}

#[test]
fn a_generator_tiles_at_its_own_integer_period_in_each_axis() {
    assert_eq!(alone(Noise::value().period(8)), Period::square(8));
    assert_eq!(
        alone(Noise::perlin().periods(4, 16)),
        Period::Tiled { u: 4, v: 16 }
    );
    assert_eq!(
        alone(Voronoi::new().periods(3, 5)),
        Period::Tiled { u: 3, v: 5 }
    );
    // Bricks and tiles carry their two counts the way a wall does: rows stack
    // in v, columns run along u — divided, in v, by the bond that offsets them.
    assert_eq!(
        alone(Bricks::new().rows(8).columns(1).offset(0.0)),
        Period::Tiled { u: 1, v: 8 },
        "a stack bond repeats every row"
    );
    assert_eq!(
        alone(Tiles::new().rows(2).columns(6)),
        Period::Tiled { u: 6, v: 2 }
    );
    assert_eq!(
        alone(Pattern::new(PatternKind::Stripes).x(4).y(1)),
        Period::Tiled { u: 4, v: 1 }
    );
    // A shape sits in the repeat, and scratches wrap at its edges.
    assert_eq!(alone(Shape::new(ShapeKind::Circle)), Period::UNIT);
    assert_eq!(alone(Scratches::new()), Period::UNIT);
}

#[test]
fn every_kind_and_output_of_a_shape_tiles_once_and_lays_no_lattice() {
    // The rule belongs to the construction rather than to the kind: every
    // shape is measured from a coordinate that was wrapped first and is kept
    // inside the repeat by its own check, so a capsule, a gear, a shell and a
    // rounded corner all tile once for the reason a disc does. The distance
    // output is the same field the mask ramps, so it tiles for the same reason
    // again. And none of them lays a lattice, so none of them asks a bake for
    // a resolution it would not otherwise have picked.
    for kind in [
        ShapeKind::Circle,
        ShapeKind::Box,
        ShapeKind::Polygon,
        ShapeKind::Star,
        ShapeKind::Capsule,
        ShapeKind::Gear,
    ] {
        for output in [ShapeOutput::Mask, ShapeOutput::Distance] {
            let shape = Shape::new(kind).round(0.05).hollow(0.1).output(output);
            assert_eq!(alone(shape), Period::UNIT, "{kind:?} as {output:?}");
            let material = MaterialGraph::builder("test:shape")
                .node("n", shape)
                .output(PbrOutput::new().roughness("n"))
                .build()
                .expect("a shape that fits its repeat");
            assert_eq!(material.finest_lattice(), [1, 1], "{kind:?} as {output:?}");
        }
    }
}

#[test]
fn a_wall_repeats_in_v_at_its_bond_rather_than_at_its_rows() {
    // Row `r` is offset along u by `r * offset` bricks, and a whole brick is
    // nothing, so the wall only comes back where the accumulated offset is
    // whole. The default running bond therefore repeats every two rows, and
    // eight rows of it tile four times, not eight: claiming eight would say the
    // wall is invariant under a shift of one row, which is exactly the shift
    // that moves every brick half a width along.
    assert_eq!(
        alone(Bricks::new().rows(8).columns(1)),
        Period::Tiled { u: 1, v: 4 },
        "a running bond over eight rows"
    );
    assert_eq!(
        alone(Bricks::new().rows(8).columns(2).offset(1.0)),
        Period::Tiled { u: 2, v: 8 },
        "a whole brick of offset is a stack bond"
    );
    assert_eq!(
        alone(Bricks::new().rows(9).columns(2).offset(1.0 / 3.0)),
        Period::Tiled { u: 2, v: 3 },
        "a third bond comes back every three rows"
    );
    // A bond that does not divide the rows never closes: row zero and the row
    // above the v seam disagree by part of a brick, which is a visible line
    // across the wall. It is a fact about the node's own fields, so it is
    // rejected there rather than inferred free and blamed on the output.
    let error = MaterialGraph::builder("test:bond")
        .node("wall", Bricks::new().rows(3).columns(2))
        .output(PbrOutput::new().base_color("wall"))
        .build()
        .expect_err("a half bond over three rows does not close");
    assert_eq!(error.path, "nodes[wall].offset");
    assert!(error.reason.contains("v seam"), "{error}");
    // And the over-claim matters downstream: everything that reads the wall
    // lands on the multiple of what the wall says, so a wall that claimed
    // `v: 8` would put a blend with a four-row noise on eight rows and warn
    // about a repeat neither input has. Four is what it really has, and four
    // and four are four.
    assert_eq!(
        period(
            vec![
                ("wall", Bricks::new().rows(8).columns(1).into()),
                ("grime", Noise::value().periods(1, 4).into()),
                (
                    "dirty",
                    Blend::new(BlendMode::Multiply, "wall", "grime").into(),
                ),
            ],
            "dirty",
        ),
        Period::Tiled { u: 1, v: 4 }
    );
}

#[test]
fn a_tiling_repeats_at_its_lattice_rather_than_at_its_grid() {
    // A grid is its own lattice, so its cells are its repeats.
    assert_eq!(
        alone(Tiles::new().pattern(TilePattern::Grid).rows(3).columns(5)),
        Period::Tiled { u: 5, v: 3 }
    );
    // A hex lattice offsets its odd rows by half a cell, which is a running
    // bond by another name: two rows are one repeat.
    assert_eq!(
        alone(Tiles::new().pattern(TilePattern::Hex).rows(6).columns(5)),
        Period::Tiled { u: 5, v: 3 }
    );
    // A herringbone weave comes back every *four* cells in each axis. Two
    // would be a bond rather than a weave: a two-by-two block of cells holds
    // two of its rectangles, and the only ways to lay two in it are stacked or
    // side by side.
    assert_eq!(
        alone(
            Tiles::new()
                .pattern(TilePattern::Herringbone)
                .rows(12)
                .columns(8)
        ),
        Period::Tiled { u: 2, v: 3 }
    );
    // Neither lattice closes over a count it does not divide, and each says
    // which field is the one to bring into line.
    for (tiles, path) in [
        (
            Tiles::new().pattern(TilePattern::Hex).rows(5).columns(4),
            "nodes[floor].rows",
        ),
        (
            Tiles::new()
                .pattern(TilePattern::Herringbone)
                .rows(4)
                .columns(6),
            "nodes[floor].columns",
        ),
    ] {
        let error = MaterialGraph::builder("test:lattice")
            .node("floor", tiles)
            .output(PbrOutput::new().base_color("floor"))
            .build()
            .expect_err("the lattice does not close over an odd count");
        assert_eq!(error.path, path);
    }
}

#[test]
fn a_weave_tiles_at_its_crossing_rather_than_at_its_threads() {
    // A plain weave crosses one way on even threads and the other way on odd
    // ones, so eight threads carry four repeats of the crossing and not eight,
    // exactly as a running bond over eight rows carries four.
    assert_eq!(alone(Weave::new().x(8).y(8)), Period::square(4));
    // The two axes are counted separately, because a cloth wider than it is
    // fine is a real thing to author.
    assert_eq!(alone(Weave::new().x(12).y(4)), Period::Tiled { u: 6, v: 2 });
    // And the divisor is the pattern's own repeat rather than two: a twill of
    // four threads over twelve tiles three times, a satin of five over ten
    // twice.
    assert_eq!(
        alone(
            Weave::new()
                .x(12)
                .y(12)
                .pattern(WeavePattern::Twill { step: 4 })
        ),
        Period::square(3)
    );
    assert_eq!(
        alone(
            Weave::new()
                .x(10)
                .y(10)
                .pattern(WeavePattern::Satin { step: 5 })
        ),
        Period::square(2)
    );
    // Every output reads the same cloth and therefore the same period, the
    // per-thread ids included: an id is a lattice rather than a shift the
    // field is invariant under, which is the reading `Period` already puts on
    // a noise's cells and a wall's per-brick id.
    for output in [
        WeaveOutput::Mask,
        WeaveOutput::Height,
        WeaveOutput::Warp,
        WeaveOutput::Weft,
        WeaveOutput::Id,
    ] {
        assert_eq!(
            alone(Weave::new().x(8).y(8).output(output)),
            Period::square(4),
            "{output:?}"
        );
    }

    // A thread count the crossing does not divide has no period at all, and it
    // is a fact about this node's own fields, so it is refused at the field
    // that decides it rather than inferred free.
    let error = MaterialGraph::builder("test:weave")
        .node("probe", Weave::new().x(7).y(8))
        .output(PbrOutput::new().roughness("probe"))
        .build()
        .expect_err("a cloth whose threads do not divide its crossing");
    assert_eq!(error.path, "nodes[probe].x");

    // The lattice is the threads themselves rather than the crossing they
    // repeat at: two threads of a plain weave are two threads to rasterise
    // whether or not the second crosses the way the first did.
    let lattice = |weave: Weave| {
        MaterialGraph::builder("test:weave")
            .node("probe", weave)
            .output(PbrOutput::new().roughness("probe"))
            .build()
            .expect("a valid graph")
            .finest_lattice()
    };
    assert_eq!(lattice(Weave::new().x(8).y(8)), [8, 8]);
    assert_eq!(lattice(Weave::new().x(12).y(4)), [12, 4]);
    assert_eq!(
        lattice(
            Weave::new()
                .x(12)
                .y(12)
                .pattern(WeavePattern::Twill { step: 4 })
        ),
        [12, 12]
    );
}

#[test]
fn a_pointwise_node_takes_the_least_common_multiple_of_its_inputs() {
    let material = probe(vec![
        ("eight", Noise::value().period(8).into()),
        ("four", Noise::value().period(4).into()),
        ("twelve", Noise::value().period(12).into()),
        (
            "divides",
            Blend::new(BlendMode::Add, "eight", "four").into(),
        ),
        ("constant", Blend::new(BlendMode::Add, "eight", 0.5).into()),
    ]);
    // Four divides eight, so nothing grew and nothing is worth saying.
    assert_eq!(
        material.port("divides").expect("inferred").period,
        Period::square(8)
    );
    assert_eq!(
        material.port("constant").expect("inferred").period,
        Period::square(8)
    );
    assert!(material.warnings().is_empty(), "{:?}", material.warnings());

    // Eight and twelve tile at twenty-four, which is legal and suspicious.
    let material = probe(vec![
        ("eight", Noise::value().period(8).into()),
        ("twelve", Noise::value().period(12).into()),
        ("mix", Blend::new(BlendMode::Add, "eight", "twelve").into()),
    ]);
    assert_eq!(
        material.port("mix").expect("inferred").period,
        Period::square(24)
    );
    let warning = material.warnings().first().expect("a warning");
    assert_eq!(warning.path, "nodes[mix]");
    assert!(warning.message.contains("24x24"), "{warning}");
}

#[test]
fn a_multiple_past_what_a_bake_can_carry_is_free_rather_than_a_lie() {
    let free = period(
        vec![
            ("wide", Noise::value().period(2048).into()),
            ("odd", Noise::value().period(3).into()),
            ("mix", Blend::new(BlendMode::Add, "wide", "odd").into()),
        ],
        "mix",
    );
    assert_eq!(free, Period::Free, "6144 repeats is past {MAX_PERIOD}");
}

#[test]
fn a_transform_scales_by_whole_repeats_and_rotates_by_quarter_turns() {
    let base = || ("noise", Node::from(Noise::value().periods(4, 8)));
    let of = |node: Transform| period(vec![base(), ("probe", node.into())], "probe");

    assert_eq!(of(Transform::new("noise")), Period::Tiled { u: 4, v: 8 });
    assert_eq!(
        of(Transform::new("noise").scale(2.0)),
        Period::Tiled { u: 8, v: 16 }
    );
    assert_eq!(
        of(Transform::new("noise").scales(3.0, 1.0)),
        Period::Tiled { u: 12, v: 8 }
    );
    // A quarter turn exchanges the axes; a half turn leaves them alone.
    assert_eq!(
        of(Transform::new("noise").rotate(90.0)),
        Period::Tiled { u: 8, v: 4 }
    );
    assert_eq!(
        of(Transform::new("noise").rotate(-90.0)),
        Period::Tiled { u: 8, v: 4 }
    );
    assert_eq!(
        of(Transform::new("noise").rotate(180.0)),
        Period::Tiled { u: 4, v: 8 }
    );
    // The coordinate is scaled before it is rotated, so the turn exchanges the
    // axes first and the scale multiplies what landed there: u takes the
    // source's 8 and doubles it. Applying the scale after the turn would give
    // 12x24 instead, which is why the order is written on the node.
    assert_eq!(
        of(Transform::new("noise").scales(2.0, 3.0).rotate(90.0)),
        Period::Tiled { u: 16, v: 12 }
    );
    // Translation is free in both senses: it costs nothing and changes nothing.
    assert_eq!(
        of(Transform::new("noise").translate(0.37, -12.5)),
        Period::Tiled { u: 4, v: 8 }
    );
    for broken in [
        Transform::new("noise").scale(1.5),
        Transform::new("noise").scale(0.5),
        Transform::new("noise").rotate(45.0),
        Transform::new("noise").scale(2048.0),
        Transform::new("noise").scale(2.0).clamped(),
        // A clamped translation is the quiet one: the source is read at
        // `u - 0.37`, which is clamped over the leftmost third of the result,
        // so the field is a constant smear there and does not meet itself.
        Transform::new("noise").translate(0.37, 0.0).clamped(),
        Transform::new("noise").translate(0.0, -0.5).clamped(),
        Transform::new("noise").rotate(90.0).clamped(),
    ] {
        assert_eq!(of(broken), Period::Free);
    }
    // Clamping only matters where the coordinate actually moved, and a whole
    // turn is the frame it started in.
    assert_eq!(
        of(Transform::new("noise").clamped()),
        Period::Tiled { u: 4, v: 8 }
    );
    assert_eq!(
        of(Transform::new("noise").rotate(360.0).clamped()),
        Period::Tiled { u: 4, v: 8 }
    );
    assert_eq!(
        of(Transform::new("noise").rotate(-720.0).clamped()),
        Period::Tiled { u: 4, v: 8 }
    );
}

#[test]
fn a_tile_wraps_its_instances_and_tiles_once_whatever_it_scattered() {
    assert_eq!(
        period(
            vec![
                ("noise", Noise::value().period(7).into()),
                ("scatter", Tile::new("noise").count(5).into()),
            ],
            "scatter",
        ),
        Period::UNIT
    );
}

#[test]
fn a_warp_keeps_its_source_where_the_offset_divides_it_and_is_free_otherwise() {
    let of = |offset: &str| {
        period(
            vec![
                ("source", Noise::value().period(8).into()),
                ("four", Noise::value().period(4).into()),
                ("three", Noise::value().period(3).into()),
                ("turned", Transform::new("four").rotate(30.0).into()),
                ("probe", Warp::new("source", offset).into()),
            ],
            "probe",
        )
    };
    // The usual case: the offset's period divides the source's, so the warp
    // inherits the source's period exactly.
    assert_eq!(of("four"), Period::square(8));
    // Where it does not, the warped field genuinely repeats at the multiple.
    assert_eq!(of("three"), Period::square(24));
    // An offset that does not tile displaces by a different amount at each
    // seam, so nothing downstream of it does either.
    assert_eq!(of("turned"), Period::Free);
}

#[test]
fn a_mirror_folds_the_whole_unit_and_keeps_the_lattice_it_folded() {
    let of = |source: u32, axis: MirrorAxis| {
        period(
            vec![
                ("noise", Noise::value().periods(source, 6).into()),
                ("probe", Mirror::new("noise").axis(axis).into()),
            ],
            "probe",
        )
    };
    // The fold is `f(t) = 0.5 - |0.5 - fract(t)|`, over the unit and not over
    // the lattice, so the cells it reflects are the cells it was handed: eight
    // along u stay eight along u, four of them now the reflections of the other
    // four. Halving them would be a claim on a shift the source never made.
    assert_eq!(of(8, MirrorAxis::U), Period::Tiled { u: 8, v: 6 });
    assert_eq!(of(8, MirrorAxis::V), Period::Tiled { u: 8, v: 6 });
    // An odd count is nothing to a fold that never pairs repeats, and folding
    // twice is still the lattice it started with.
    assert_eq!(of(3, MirrorAxis::U), Period::Tiled { u: 3, v: 6 });
    assert_eq!(of(1, MirrorAxis::U), Period::Tiled { u: 1, v: 6 });
    assert_eq!(
        period(
            vec![
                ("noise", Noise::value().period(8).into()),
                ("once", Mirror::new("noise").into()),
                ("twice", Mirror::new("once").into()),
            ],
            "twice",
        ),
        Period::square(8)
    );
    // The fold gives the axis it folded a wrap whatever reached it, but it
    // leaves the other axis as free as it found it, and a period is free in
    // both axes or in neither.
    assert_eq!(
        period(
            vec![
                ("noise", Noise::value().period(4).into()),
                ("spun", Kaleidoscope::new("noise").count(3).into()),
                ("probe", Mirror::new("spun").into()),
            ],
            "probe",
        ),
        Period::Free
    );
}

#[test]
fn an_operand_a_unary_operator_never_reads_leaves_the_period_alone() {
    let of = |op| {
        period(
            vec![
                ("three", Noise::value().period(3).into()),
                ("four", Noise::value().period(4).into()),
                ("probe", Math::new(op, "three", "four").into()),
            ],
            "probe",
        )
    };
    // A binary operator lands on the multiple of both operands.
    assert_eq!(of(MathOp::Mul), Period::square(12));
    // A unary one never reads `b`, so `b` is not part of what its answer is
    // laid on: an absolute value of a field of three cells has three cells.
    assert_eq!(of(MathOp::Abs), Period::square(3));
}

#[test]
fn only_a_four_sector_kaleidoscope_tiles() {
    let of = |count: u32| {
        period(
            vec![
                ("noise", Noise::value().period(4).into()),
                ("probe", Kaleidoscope::new("noise").count(count).into()),
            ],
            "probe",
        )
    };
    assert_eq!(of(4), Period::UNIT);
    for count in [1, 2, 3, 5, 6, 8] {
        assert_eq!(of(count), Period::Free, "{count} sectors");
    }
    // The quadrant fold runs out and back within each axis, so it reads the
    // same value at 1 as at 0 whatever it folded — including a source that does
    // not tile at all, which is the one case where the fold gives a period back
    // to a field that had none.
    assert_eq!(
        period(
            vec![
                ("noise", Noise::value().period(4).into()),
                ("skewed", Transform::new("noise").rotate(30.0).into()),
                ("probe", Kaleidoscope::new("skewed").into()),
            ],
            "probe",
        ),
        Period::UNIT
    );
}

#[test]
fn a_circle_map_tiles_once_and_wants_a_source_that_tiles() {
    // A disc centred in the repeat with a fill outside it, read through a
    // wrapped coordinate: the count is one whatever lattice the source was
    // laid on, which is the arm a shape takes and for the same reason.
    let of = |source: Node| {
        period(
            vec![
                ("source", source),
                ("probe", CircleMap::new("source").into()),
            ],
            "probe",
        )
    };
    assert_eq!(of(Noise::value().periods(4, 16).into()), Period::UNIT);
    assert_eq!(
        of(Pattern::new(PatternKind::Stripes).x(3).into()),
        Period::UNIT
    );

    // The angular axis wraps into the source's u, so a source that does not
    // tile is refused at the node rather than answered freely: the map has no
    // period to inherit and the seam at angle zero has nowhere to hide.
    let error = MaterialGraph::builder("test:circle")
        .node("noise", Noise::value().period(4))
        .node("skewed", Transform::new("noise").rotate(30.0))
        .node("probe", CircleMap::new("skewed"))
        .output(PbrOutput::new().roughness("probe"))
        .build()
        .expect_err("a circle map over a source that does not tile");
    assert_eq!(error.path, "nodes[probe].inputs[input]");

    // The lattice is the finer of the two axes, in both axes, because the node
    // mixes them. Eight cells of the source's u go round the ring `turns`
    // times; eight of its v are squeezed into a window `radius - inner` wide,
    // and a window half the repeat across therefore wants sixteen.
    let lattice = |map: CircleMap| {
        MaterialGraph::builder("test:circle")
            .node("source", Noise::value().period(8))
            .node("probe", map)
            .output(PbrOutput::new().roughness("probe"))
            .build()
            .expect("a valid graph")
            .finest_lattice()
    };
    assert_eq!(lattice(CircleMap::new("source")), [16, 16]);
    assert_eq!(lattice(CircleMap::new("source").turns(3)), [24, 24]);
    assert_eq!(lattice(CircleMap::new("source").radius(0.25)), [32, 32]);
    assert_eq!(
        lattice(CircleMap::new("source").inner(0.25).rings(2)),
        [64, 64]
    );
}

#[test]
fn a_circle_splatter_tiles_once_unless_its_mask_does_not() {
    // A ring of instances laid inside the repeat, read through a wrapped
    // coordinate and held there by the node's own check: the count is one
    // whatever lattice the source was laid on, which is the arm a scatter takes
    // and for the same reason.
    assert_eq!(
        period(
            vec![
                ("noise", Noise::value().periods(4, 16).into()),
                ("probe", CircleSplatter::new("noise").into()),
            ],
            "probe",
        ),
        Period::UNIT
    );

    // The mask is the one input read outside an instance, so it is the one that
    // can take the period away. It is the rule a `Tile` lives under, held in
    // common rather than rediscovered.
    let masked = |mask: &str| {
        period(
            vec![
                ("noise", Noise::value().period(4).into()),
                ("skewed", Transform::new("noise").rotate(30.0).into()),
                ("probe", CircleSplatter::new("noise").mask(mask).into()),
            ],
            "probe",
        )
    };
    assert_eq!(masked("noise"), Period::UNIT);
    assert_eq!(masked("skewed"), Period::Free);

    // The lattice is the source squeezed into one instance, per axis, or the
    // mask's where that is finer: eight cells drawn a quarter of the repeat
    // across want thirty-two, and the mask is read at the instances rather than
    // inside them and lays its own.
    let lattice = |splatter: CircleSplatter, fine: bool| {
        // The finest lattice is taken over every node of the graph, read or
        // not, so the fine mask only joins the graph in the case that binds it.
        let mut builder =
            MaterialGraph::builder("test:splatter").node("source", Noise::value().period(8));
        if fine {
            builder = builder.node("fine", Noise::value().period(64).seed(2));
        }
        builder
            .node("probe", splatter)
            .output(PbrOutput::new().roughness("probe"))
            .build()
            .expect("a valid graph")
            .finest_lattice()
    };
    let ring = || CircleSplatter::new("source").radius(0.2);
    assert_eq!(lattice(ring().scale(0.5), false), [16, 16]);
    assert_eq!(lattice(ring().scale(0.25), false), [32, 32]);
    assert_eq!(lattice(ring().scale(0.2), false), [40, 40]);
    assert_eq!(lattice(ring().scale(0.5).mask("fine"), true), [64, 64]);
}

#[test]
fn a_boolean_between_two_distances_is_pointwise_and_so_is_the_mask_over_it() {
    // Neither node reads anything anywhere but the texel asking, so both take
    // the rule every pointwise filter takes — the least common multiple of the
    // inputs they read, and the largest lattice that reached them. There is no
    // arm for either in `node_period` or in `node_lattice`, and this test is
    // what says that is the right answer rather than an oversight: a boolean
    // over a field of four and a field of six comes back to itself at twelve.
    let material = probe(vec![
        ("four", Noise::value().period(4).into()),
        ("six", Noise::value().period(6).into()),
        (
            "both",
            SdfCombine::new(SdfOp::Union, "four", "six")
                .smooth(0.1)
                .into(),
        ),
        ("masked", SdfMask::new("both").into()),
        ("alone", SdfMask::new("four").into()),
    ]);
    assert_eq!(
        material.port("both").expect("inferred").period,
        Period::square(12)
    );
    assert_eq!(
        material.port("masked").expect("inferred").period,
        Period::square(12)
    );
    assert_eq!(
        material.port("alone").expect("inferred").period,
        Period::square(4)
    );

    // And a free field carries its freedom through both of them, which is what
    // stops a boolean laundering a source the crate refused to promise
    // anything about.
    let free = period(
        vec![
            (
                "skewed",
                Kaleidoscope::new(Noise::value().period(4)).count(3).into(),
            ),
            ("tiled", Noise::value().period(4).into()),
            (
                "cut",
                SdfCombine::new(SdfOp::Subtract, "tiled", "skewed").into(),
            ),
            ("masked", SdfMask::new("cut").into()),
        ],
        "masked",
    );
    assert_eq!(free, Period::Free);

    // The lattice is the largest that reached the node, as a blend's is: a
    // boolean draws no features of its own, and a ramp over one draws the same
    // features the distance already had.
    let lattice = MaterialGraph::builder("test:sdf")
        .node("coarse", Noise::value().period(4))
        .node("fine", Noise::value().period(32))
        .node("both", SdfCombine::new(SdfOp::Intersect, "coarse", "fine"))
        .node("mask", SdfMask::new("both"))
        .output(PbrOutput::new().roughness("mask"))
        .build()
        .expect("a valid graph");
    assert_eq!(lattice.finest_lattice(), [32, 32]);
}

#[test]
fn a_buffered_filter_is_wrapped_and_keeps_the_period_it_was_given() {
    let source = ("noise", Node::from(Noise::value().periods(4, 16)));
    for node in [
        Node::from(Blur::new("noise")),
        OcclusionFromHeight::new("noise").into(),
        Distance::new("noise").into(),
        Erode::new("noise").into(),
        Dilate::new("noise").into(),
        Buffer::new("noise").into(),
        // Not buffered, but it reads its neighbours through the same wrap and
        // keeps what it was given, so the rule is the same one.
        NormalFromHeight::new("noise").into(),
    ] {
        let kind = node.kind();
        assert_eq!(
            period(vec![source.clone(), ("probe", node)], "probe"),
            Period::Tiled { u: 4, v: 16 },
            "{kind}",
        );
    }
}

#[test]
fn the_material_tiles_at_the_multiple_of_every_output_it_binds() {
    let material = MaterialGraph::builder("test:output")
        .node("colour", Noise::value().period(4))
        .node("rough", Noise::value().period(6))
        .output(
            PbrOutput::new()
                .base_color("colour")
                .roughness("rough")
                .metallic(0.0),
        )
        .build()
        .expect("valid");
    assert_eq!(material.period(), Period::square(12));
    assert_eq!(
        material.output_port("metallic").expect("bound").period,
        Period::UNIT
    );
}

#[test]
fn outputs_that_tile_separately_but_share_no_multiple_a_bake_can_hold_are_rejected() {
    // Each of these tiles on its own, and each output port is happy; it is
    // only the multiple across them, 53040 repeats, that no texture can carry.
    // Letting this build would hand the bake a material with no repeat count.
    let error = MaterialGraph::builder("test:coprime")
        .node("colour", Noise::value().period(17))
        .node("rough", Noise::value().period(16))
        .node("tall", Noise::value().period(15))
        .node("glow", Noise::value().period(13))
        .output(
            PbrOutput::new()
                .base_color("colour")
                .roughness("rough")
                .height("tall")
                .emissive("glow"),
        )
        .build()
        .expect_err("no bake can hold the multiple of these four");
    // Reported at the port that ran the multiple past what a bake holds — the
    // first three still fit, at 4080 repeats — so an author knows which output
    // to bring into line.
    assert_eq!(error.path, "output.emissive");
    assert!(
        error.reason.contains(&MAX_PERIOD.to_string()),
        "{}",
        error.reason
    );
    // The same four periods are fine one at a time, so this really is about
    // the combination rather than about any one of them.
    for period in [17, 16, 15, 13] {
        assert_eq!(alone(Noise::value().period(period)), Period::square(period));
    }
    // And a material that does build has a period a bake can use.
    let material = MaterialGraph::builder("test:coprime")
        .node("colour", Noise::value().period(17))
        .node("rough", Noise::value().period(16))
        .output(PbrOutput::new().base_color("colour").roughness("rough"))
        .build()
        .expect("17 and 16 tile together at 272");
    assert_eq!(material.period(), Period::square(272));
}

/// A library of compounds that read a signal input, for the rules below.
///
/// `pass` hands its input straight to an output, so the period at the node is
/// the period of what was bound. `scaled` reads the same input through a
/// transform of scale two, which is the case a cache keyed by the graph alone
/// gets wrong: the inner frame doubles whatever it was given, and only
/// inferring the instance under the binding says so.
fn compounds() -> MaterialGraphLibrary {
    let mut library = MaterialGraphLibrary::default();
    library.insert(
        MaterialGraph::builder("test:pass")
            .node("field", GraphInput::float("field", 0.5))
            .output(PbrOutput::new().roughness("field"))
            .into_graph(),
    );
    library.insert(
        MaterialGraph::builder("test:scaled")
            .node("field", GraphInput::float("field", 0.5))
            .node("scaled", Transform::new("field").scale(2.0))
            .output(PbrOutput::new().roughness("scaled"))
            .into_graph(),
    );
    library
}

/// The material a `test:` compound is instanced by, with `grain` bound to its
/// one input.
fn instancing(key: &str, grain: impl Into<Node>) -> Result<Material, GraphError> {
    MaterialGraph::builder("test:instance")
        .node("grain", grain)
        .node(
            "inner",
            Subgraph::new(key)
                .input("field", "grain")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&compounds())
}

#[test]
fn a_bound_input_carries_the_period_of_the_field_bound_to_it() {
    // Unbound, an input is its default literal, which is one value over the
    // whole repeat and so comes back to itself once, the way a parameter does.
    assert_eq!(alone(GraphInput::float("field", 0.5)), Period::UNIT);
    let unbound = MaterialGraph::builder("test:unbound")
        .node("inner", Subgraph::new("test:pass"))
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&compounds())
        .expect("a compound builds with nothing wired into it");
    assert_eq!(unbound.period(), Period::UNIT);

    // Bound, it is the field: the period crosses the boundary and the node the
    // caller reads carries it.
    let material = instancing("test:pass", Noise::value().period(8)).expect("valid");
    assert_eq!(
        material.port("inner").expect("inferred").period,
        Period::square(8)
    );
    assert_eq!(material.period(), Period::square(8));
    assert_eq!(material.finest_lattice(), [8, 8]);
}

#[test]
fn what_an_instance_does_to_a_bound_field_is_inferred_under_the_binding() {
    // Eight cells read through a frame of scale two lay sixteen. The inner
    // graph on its own knows nothing of the eight, so this number exists only
    // because the instance was inferred again under what the node bound.
    let material = instancing("test:scaled", Noise::value().period(8)).expect("valid");
    assert_eq!(
        material.port("inner").expect("inferred").period,
        Period::square(16)
    );
    assert_eq!(material.finest_lattice(), [16, 16]);
    // And the same compound over a different field is a different answer,
    // which is what specialising per binding buys.
    let coarser = instancing("test:scaled", Noise::value().period(3)).expect("valid");
    assert_eq!(
        coarser.port("inner").expect("inferred").period,
        Period::square(6)
    );
}

#[test]
fn a_free_field_bound_into_a_compound_is_refused_at_the_node_that_bound_it() {
    // The field does not tile, the compound hands it to an output, and a
    // material output must tile. The node that broke the instance is in the
    // outer graph, and its author may never have opened the inner one, so the
    // refusal is at their node and the inner path rides in the message.
    let error = MaterialGraph::builder("test:instance")
        .node("grain", Noise::value().period(4))
        .node("turned", Transform::new("grain").rotate(33.0))
        .node(
            "inner",
            Subgraph::new("test:pass")
                .input("field", "turned")
                .output(SurfaceOutput::Roughness),
        )
        .output(PbrOutput::new().roughness("inner"))
        .into_graph()
        .build_in(&compounds())
        .expect_err("a free field reaches the compound's output");
    assert_eq!(error.path, "nodes[inner]");
    assert!(
        error
            .reason
            .contains("does not build with the fields this node binds"),
        "{error}"
    );
    // The inner path names the instance the binding made, hash and all, and
    // the node inside it that could not tile.
    assert!(error.reason.contains("graphs[test:pass@"), "{error}");
    assert!(error.reason.contains("nodes[field]"), "{error}");
    assert!(error.reason.contains("must tile"), "{error}");
}
