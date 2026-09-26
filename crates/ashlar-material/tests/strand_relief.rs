//! What a `StrandRelief` splats, and the promises the plane makes.
//!
//! A relief is the one plane in this crate that is not a filter over a
//! sub-expression: it is the *same* scatter the geometry is built from, seen
//! from directly above. So the cases here are about the two halves agreeing —
//! a texel over a kept strand's root is covered, a density of zero covers
//! nothing — and about the plane being a plane like any other: wrapped at the
//! seam, the same bytes however the rows were divided, and keyed by everything
//! that decides it.
//!
//! The golden hashes at the end are the same kind of pin the filter digests in
//! `planes.rs` are, and for the same reason: the splat is `f32` arithmetic in a
//! fixed order, and a change to that order is a change to every shipped map
//! whether or not anyone meant one.
#![allow(
    clippy::unwrap_used,
    reason = "fixtures built in the test; a failure is a test failure"
)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "texel coordinates of a 256-texel plane, wrapped before they are used"
)]
#![allow(
    clippy::float_cmp,
    reason = "a splat that wrapped writes the *same* texel two ways, and a lawn \
              that kept no strands covers exactly nothing; a margin would pass a \
              splat that had quietly stopped doing either"
)]
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use ashlar_material::{
    Input, MaterialGraph, MaterialGraphLibrary, ParamValue, PbrOutput, StrandLayer,
    bake::{BakeRequest, rasterise},
    interp::Plane,
    ir::{Target, lower},
    nodes::{Distance, Math, MathOp, Noise, StrandRelief, StrandReliefOutput, Uv},
    planes::{BakeCache, plane_keys, rasterise_buffers},
    strands::{StrandRequest, scatter},
};

/// Texels per repeat for the planes below.
///
/// The smallest a bake allows, and fine enough for the layer's own blade: a
/// splat coarser than a blade is what the lattice warning is about, and every
/// case but that one wants a plane that can actually draw one.
const N: u32 = 256;

/// One metre per repeat, so a length in metres is a length in UV and a claim
/// about where a footprint lands can be worked out by hand.
const REPEAT: f32 = 1.0;

fn threads(count: usize) -> Option<NonZeroUsize> {
    NonZeroUsize::new(count)
}

fn no_params() -> BTreeMap<String, ParamValue> {
    BTreeMap::new()
}

/// A graph whose roughness is one relief output of one layer of leaning blades.
///
/// Leaning, because a strand standing straight up casts a disc the width of
/// itself and there would be nothing to test a footprint with. The density is a
/// noise so that the lattice is not uniformly full, which is what makes the
/// seam and subset cases about something.
fn lawn(output: StrandReliefOutput, density: impl Into<Input>) -> MaterialGraph {
    MaterialGraph::builder("test:lawn")
        .node("patches", Noise::value().period(4).seed(7))
        .node("relief", StrandRelief::new("blades", REPEAT).output(output))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(16)
                .seed(3)
                .density(density)
                .metres(0.12, 0.02)
                .lean(0.5)
                .bend(0.4)
                .direction(Input::vec2([1.0, 0.0]))
                .colors([0.1, 0.2, 0.0], [0.6, 0.8, 0.1])
                .segments(3),
        )
        .into_graph()
}

/// The roughness plane of a graph, at `N`, over `count` threads.
fn splatted(graph: &MaterialGraph, count: usize) -> Vec<f32> {
    rasterise(&BakeRequest {
        graph,
        library: &MaterialGraphLibrary::default(),
        params: &no_params(),
        resolution: N,
        mips: false,
        threads: threads(count),
    })
    .unwrap()
    .0
    .roughness
}

/// One texel of a square plane held as a flat vector, wrapped.
fn at(lanes: &[f32], x: i64, y: i64) -> f32 {
    let size = i64::from(N);
    let index = (y.rem_euclid(size) * size + x.rem_euclid(size)) as usize;
    lanes.get(index).copied().unwrap_or_default()
}

/// FNV-1a over the bits of every lane, in order: the same digest
/// `planes.rs` pins its filters with.
fn digest(lanes: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for lane in lanes {
        for byte in lane.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn coverage_is_non_zero_at_every_kept_strands_root() {
    // The claim the whole node exists for: the relief and the geometry are the
    // same strands. So every root the scatter kept is a texel the splat wrote,
    // and a relief that had quietly become a second drawing would fail here.
    let graph = lawn(StrandReliefOutput::Coverage, "patches");
    let set = scatter(&StrandRequest {
        graph: &graph,
        library: &MaterialGraphLibrary::default(),
        params: &no_params(),
        layer: "blades",
        field_resolution: N,
        threads: threads(2),
    })
    .unwrap();
    assert!(!set.is_empty(), "the fixture keeps some strands");
    let plane = splatted(&graph, 2);
    let mut bare = Vec::new();
    for strand in set.strands() {
        let x = (strand.root[0] * f32::from(u16::try_from(N).unwrap())) as i64;
        let y = (strand.root[1] * f32::from(u16::try_from(N).unwrap())) as i64;
        // The root's own texel, or one of the eight around it: a root lands
        // somewhere inside a texel rather than on its centre, and a blade two
        // texels wide covers the centre it is nearest.
        let covered = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
            .any(|(dx, dy)| at(&plane, x + dx, y + dy) > 0.0);
        if !covered {
            bare.push(strand.root);
        }
    }
    assert!(
        bare.is_empty(),
        "roots with no coverage under them: {bare:?}"
    );
}

#[test]
fn a_buffered_field_is_read_at_one_resolution_whatever_the_bake_asks_for() {
    // The disagreement this node exists to remove, in the one place it can
    // still hide. A field that reaches a buffered filter is a *plane*, and a
    // plane has a size. If the relief rasterised that plane at the bake's
    // resolution while the mesh builder rasterised it at its own, the two
    // halves would scatter *different strands* — and the blades in the texture
    // would not be the blades standing on it, which is the whole claim.
    //
    // A jump-flood distance is what makes the case bite: it is the filter whose
    // answer moves most with the grid, so the set it gates really does change
    // between 256 and 512. The blades are narrow and upright, so a footprint is
    // a small disc over its own root and "covered" means "this strand is in the
    // set" rather than "a neighbour reached it".
    let graph = MaterialGraph::builder("test:buffered")
        .node("speckle", Noise::value().period(16).seed(11))
        .node("far", Distance::new("speckle").threshold(0.55).range(0.25))
        .node("relief", StrandRelief::new("blades", REPEAT))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(32)
                .seed(3)
                .density("far")
                .metres(0.02, 0.01),
        )
        .into_graph();
    let sown = |resolution: u32| {
        scatter(&StrandRequest {
            graph: &graph,
            library: &MaterialGraphLibrary::default(),
            params: &no_params(),
            layer: "blades",
            field_resolution: resolution,
            threads: threads(2),
        })
        .unwrap()
    };
    // The premise: this layer really is a different lawn at the two sizes, so
    // the assertions below are about something.
    let coarse = sown(N);
    let geometry = sown(ashlar_material::strands::FIELD_RESOLUTION);
    assert_ne!(
        N,
        ashlar_material::strands::FIELD_RESOLUTION,
        "the bake and the scatter must differ for this case to mean anything"
    );
    assert_ne!(
        coarse.len(),
        geometry.len(),
        "a distance-gated layer should keep a different set at a different grid"
    );

    // The relief, splatted inside a bake at `N` — the *coarse* resolution — is
    // the set the geometry planted, not the set this bake's own grid would give.
    let plane = splatted(&graph, 2);
    let covered = |root: [f32; 2]| {
        let x = (root[0] * f32::from(u16::try_from(N).unwrap())) as i64;
        let y = (root[1] * f32::from(u16::try_from(N).unwrap())) as i64;
        (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
            .any(|(dx, dy)| at(&plane, x + dx, y + dy) > 0.0)
    };
    let bare: Vec<[f32; 2]> = geometry
        .strands()
        .iter()
        .map(|strand| strand.root)
        .filter(|root| !covered(*root))
        .collect();
    assert!(
        bare.is_empty(),
        "roots the geometry planted that the relief did not splat: {bare:?}"
    );
    // And the other direction, which is what actually fails when the relief
    // reads its planes at the bake's size: a strand only the coarse grid keeps
    // must not be in the relief.
    let roots: Vec<[f32; 2]> = geometry.strands().iter().map(|s| s.root).collect();
    let intruders = coarse
        .strands()
        .iter()
        .filter(|strand| !roots.contains(&strand.root))
        .filter(|strand| covered(strand.root))
        .count();
    assert_eq!(
        intruders, 0,
        "the relief splatted strands only this bake's own grid would keep"
    );
}

#[test]
fn a_density_of_zero_covers_nothing_at_all() {
    // A threshold rather than a fade, all the way through: no strand is kept,
    // so no footprint is splatted, so the plane is exactly zero — not a dim
    // lawn, which is what a coverage built out of the density field itself
    // would have answered.
    let graph = lawn(StrandReliefOutput::Coverage, 0.0);
    let plane = splatted(&graph, 2);
    assert!(
        plane.iter().all(|value| *value == 0.0),
        "a layer that keeps no strands splats nothing"
    );
}

#[test]
fn the_splat_wraps_at_both_seams() {
    // A footprint that runs off one edge comes back on the other, so the
    // columns either side of the seam are as related as any other neighbouring
    // pair — and a strand rooted hard against the seam contributes to both
    // sides of it. A splat that clipped instead of wrapping leaves a blank
    // column here and a perfect period everywhere period inference looks.
    let graph = lawn(StrandReliefOutput::Coverage, 1.0);
    let plane = splatted(&graph, 2);
    let size = i64::from(N);
    for index in 0..size {
        // `at` wraps, so these are the same texel read two ways: what this
        // pins is that the plane the splat wrote is the plane a wrapped read
        // of it describes, which is only true if the stamps wrapped too.
        assert_eq!(at(&plane, -1, index), at(&plane, size - 1, index));
        assert_eq!(at(&plane, index, -1), at(&plane, index, size - 1));
    }
    // And the seam is not a hole: a lawn at full density covers the first
    // column as much as it covers the middle.
    let column = |x: i64| (0..size).map(|y| at(&plane, x, y)).sum::<f32>();
    let edge = column(0) + column(size - 1);
    let middle = column(size / 2) + column(size / 2 + 1);
    assert!(
        edge > 0.5 * middle,
        "the seam columns are bare: {edge} against {middle}"
    );
}

#[test]
fn a_strand_that_crosses_the_seam_contributes_on_both_sides() {
    // One strand, rooted just inside the right-hand edge and leaning along
    // `+u`, so its footprint has to run off that edge and come back on the
    // left. The layer is one cell with no jitter, which is what puts the root
    // where the case needs it.
    let graph = MaterialGraph::builder("test:seam")
        .node("uv", Uv::new())
        .node("relief", StrandRelief::new("blades", REPEAT))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(1)
                .jitter(0.0)
                .metres(0.9, 0.05)
                .lean(1.0)
                .direction(Input::vec2([1.0, 0.0])),
        )
        .into_graph();
    let plane = splatted(&graph, 2);
    // The one cell's centre is the middle of the repeat and the blade lies
    // flat along `+u` for nine tenths of one, so it runs past the seam.
    let right = (0..i64::from(N))
        .map(|y| at(&plane, i64::from(N) - 1, y))
        .sum::<f32>();
    let left = (0..i64::from(N)).map(|y| at(&plane, 0, y)).sum::<f32>();
    assert!(right > 0.0, "the strand reaches the right-hand edge");
    assert!(left > 0.0, "and comes back on the left: {left}");
}

#[test]
fn the_splat_does_not_depend_on_how_the_rows_were_divided() {
    // The plane is accumulated per row and the *rows* are what is divided, so
    // every texel takes its contributions in the one order the set is sorted
    // in. A splat that divided the strands instead would reduce partial planes
    // and `a + b` of two coverages is not `b + a` in an `f32`.
    for output in [
        StrandReliefOutput::Coverage,
        StrandReliefOutput::Height,
        StrandReliefOutput::Along,
        StrandReliefOutput::Id,
    ] {
        let graph = lawn(output, "patches");
        let one = splatted(&graph, 1);
        for count in [2, 3, 7] {
            assert_eq!(
                one.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                splatted(&graph, count)
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                "{output:?} differs across {count} threads"
            );
        }
    }
}

#[test]
fn a_height_is_zero_off_the_blades_and_never_past_the_layers_own_length() {
    // The height is in units of `length_metres`, which is what lets a graph
    // add it straight to a height field: one is a full-length strand standing
    // straight up, and bare surface is zero rather than the height of a strand
    // that is not there.
    let graph = lawn(StrandReliefOutput::Height, "patches");
    let plane = splatted(&graph, 2);
    assert!(plane.contains(&0.0), "bare bed is zero");
    assert!(
        plane.iter().all(|value| (0.0..=1.0).contains(value)),
        "a leaning strand never reaches its own length"
    );
}

#[test]
fn mass_accumulates_where_coverage_saturates_and_occlusion_weighs_it_by_height() {
    // The two outputs phase 3b added, and the reason they are not the coverage
    // under another name. Coverage is a *union* and stops at one, which is the
    // right answer to "is there a blade here" and the wrong one to "how much
    // grass is here": ten blades crossing read the same as one. Mass keeps
    // counting, which is what a graph builds the pile out of.
    let dense = |per_cell: u32, output| {
        let graph = MaterialGraph::builder("test:mat")
            .node("relief", StrandRelief::new("blades", REPEAT).output(output))
            .output(PbrOutput::new().roughness("relief"))
            .strands(
                "blades",
                StrandLayer::new()
                    .count(24)
                    .per_cell(per_cell)
                    .seed(5)
                    .metres(0.25, 0.02)
                    .lean(0.8)
                    .bend(0.2)
                    .direction(Input::vec2([1.0, 0.0])),
            )
            .into_graph();
        splatted(&graph, 2)
    };
    let coverage = dense(1, StrandReliefOutput::Coverage);
    let mass = dense(1, StrandReliefOutput::Mass);
    let occlusion = dense(1, StrandReliefOutput::Occlusion);

    // Where a texel is covered at all, there is mass, and the mass is at least
    // the coverage: a union of alphas never exceeds their sum.
    for (cover, mass) in coverage.iter().zip(&mass) {
        assert!(
            mass + 1e-5 >= *cover,
            "coverage {cover} over mass {mass}: a union cannot beat its own sum"
        );
    }
    // And it is genuinely unsaturated: a lawn this dense piles blades over one
    // another, so somewhere the sum is past what coverage can ever reach.
    let piled = mass.iter().filter(|value| **value > 1.0).count();
    assert!(
        piled > 0,
        "nothing overlapped, so nothing is being measured"
    );
    assert!(
        coverage.iter().all(|value| *value <= 1.0 + 1e-6),
        "coverage saturates and mass does not"
    );
    // Thicker still is more mass, which is the claim the pile rests on.
    let thicker = dense(3, StrandReliefOutput::Mass);
    let total = |lanes: &[f32]| lanes.iter().map(|v| f64::from(*v)).sum::<f64>();
    assert!(
        total(&thicker) > total(&mass),
        "three strands a cell is no more grass than one"
    );

    // Occlusion is that same accumulation weighted by the height each
    // contribution stood at, so it is bounded above by the mass — every height
    // here is at most one — and it is zero exactly where nothing covered.
    for (mass, shade) in mass.iter().zip(&occlusion) {
        assert!(
            *shade <= mass + 1e-5,
            "occlusion {shade} over mass {mass}: heights are at most one"
        );
        if *mass == 0.0 {
            assert_eq!(*shade, 0.0, "bare ground occludes nothing");
        }
    }
    assert!(
        occlusion.iter().any(|value| *value > 0.0),
        "a pile of leaning blades occludes something"
    );
}

#[test]
fn a_relief_never_writes_a_height_below_the_surface() {
    // A strand whose bend carries its tip past horizontal is drooping onto the
    // ground, and a sunk root starts under it. Both leave the half-space the
    // relief describes, and a plane that carried the negative would read as a
    // trench where a blade lay down — so a graph adding it to a bed would dig
    // one.
    let graph = MaterialGraph::builder("test:droop")
        .node(
            "relief",
            StrandRelief::new("blades", REPEAT).output(StrandReliefOutput::Height),
        )
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(12)
                .metres(0.3, 0.02)
                // Leaning most of a quarter turn and bending the rest of the
                // way, so the tip is well past horizontal.
                .lean(0.9)
                .bend(1.0)
                .height_offset(0.12)
                .direction(Input::vec2([1.0, 0.0])),
        )
        .into_graph();
    let plane = splatted(&graph, 2);
    assert!(
        plane.iter().all(|value| *value >= 0.0),
        "a drooping or buried strand dug a trench: {:?}",
        plane.iter().copied().fold(f32::INFINITY, f32::min)
    );
    assert!(
        plane.iter().any(|value| *value > 0.0),
        "and it still stands somewhere"
    );
}

#[test]
fn a_plane_key_covers_the_layers_fields_its_constants_and_the_node() {
    // The key has to name everything a splat depends on and nothing else,
    // because what it protects is a cache handed across bakes: a lawn whose
    // density field changed and whose key did not is the previous lawn, drawn
    // over the new one's geometry.
    let keys = |graph: MaterialGraph| {
        let material = graph.build_in(&MaterialGraphLibrary::default()).unwrap();
        let ir = lower(&material, Target::Bake).unwrap();
        plane_keys(&ir, N)
    };
    let base = keys(lawn(StrandReliefOutput::Coverage, "patches"));
    assert_eq!(base, keys(lawn(StrandReliefOutput::Coverage, "patches")));

    // A different field under the same node.
    assert_ne!(base, keys(lawn(StrandReliefOutput::Coverage, 1.0)));
    // A different output of the same layer.
    assert_ne!(base, keys(lawn(StrandReliefOutput::Height, "patches")));

    // A constant of the layer that no expression reaches, which is the case a
    // key built out of the lowered fields alone would miss.
    let widened = MaterialGraph::builder("test:lawn")
        .node("patches", Noise::value().period(4).seed(7))
        .node("relief", StrandRelief::new("blades", REPEAT))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(16)
                .seed(3)
                .density("patches")
                .metres(0.12, 0.03)
                .lean(0.5)
                .bend(0.4)
                .direction(Input::vec2([1.0, 0.0]))
                .colors([0.1, 0.2, 0.0], [0.6, 0.8, 0.1])
                .segments(3),
        )
        .into_graph();
    assert_ne!(base, keys(widened), "a wider blade is a different plane");

    // And every constant phase 3b added, each of which moves the set or the
    // footprint. A key that named none of them would hand a cached plane of the
    // old lawn to the new one.
    let tweaked = |change: fn(StrandLayer) -> StrandLayer| {
        MaterialGraph::builder("test:lawn")
            .node("patches", Noise::value().period(4).seed(7))
            .node("relief", StrandRelief::new("blades", REPEAT))
            .output(PbrOutput::new().roughness("relief"))
            .strands(
                "blades",
                change(
                    StrandLayer::new()
                        .count(16)
                        .seed(3)
                        .density("patches")
                        .metres(0.12, 0.02)
                        .lean(0.5)
                        .bend(0.4)
                        .direction(Input::vec2([1.0, 0.0]))
                        .colors([0.1, 0.2, 0.0], [0.6, 0.8, 0.1])
                        .segments(3),
                ),
            )
            .into_graph()
    };
    for (what, change) in [
        (
            "per_cell",
            (|l: StrandLayer| l.per_cell(3)) as fn(StrandLayer) -> StrandLayer,
        ),
        ("midpoint", |l: StrandLayer| l.midpoint(0.4)),
        ("facing", |l: StrandLayer| l.facing_variation(0.7)),
        ("sink", |l: StrandLayer| l.height_offset(0.01)),
        ("clumps", |l: StrandLayer| l.clumps(4, 0.5, 0.5)),
        ("tint", |l: StrandLayer| l.clump_tint(0.4)),
    ] {
        assert_ne!(base, keys(tweaked(change)), "{what} is not in the key");
    }

    // And the repeat the node is measured against, which is on the node rather
    // than on the layer.
    let stretched = MaterialGraph::builder("test:lawn")
        .node("patches", Noise::value().period(4).seed(7))
        .node("relief", StrandRelief::new("blades", 2.0))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(16)
                .seed(3)
                .density("patches")
                .metres(0.12, 0.02)
                .lean(0.5)
                .bend(0.4)
                .direction(Input::vec2([1.0, 0.0]))
                .colors([0.1, 0.2, 0.0], [0.6, 0.8, 0.1])
                .segments(3),
        )
        .into_graph();
    assert_ne!(
        base,
        keys(stretched),
        "a larger repeat is a different plane"
    );
}

#[test]
fn two_outputs_of_one_layer_are_two_planes_over_one_scatter() {
    // The sharing the cache exists for, and the one a key that named only the
    // filter would have got wrong in the other direction: coverage and height
    // are two different planes, and both come off one scatter.
    let graph = MaterialGraph::builder("test:two")
        .node("cover", StrandRelief::new("blades", REPEAT))
        .node(
            "high",
            StrandRelief::new("blades", REPEAT).output(StrandReliefOutput::Height),
        )
        .node("both", Math::new(MathOp::Add, "cover", "high"))
        .output(PbrOutput::new().roughness("both"))
        .strands(
            "blades",
            StrandLayer::new().count(8).metres(0.2, 0.03).lean(0.6),
        )
        .into_graph();
    let material = graph.build_in(&MaterialGraphLibrary::default()).unwrap();
    let ir = lower(&material, Target::Bake).unwrap();
    assert_eq!(ir.buffers().len(), 2, "two outputs, two planes");
    let keys = plane_keys(&ir, N);
    assert_ne!(keys[0], keys[1]);
    let scatters: Vec<&[u8]> = ir
        .buffers()
        .iter()
        .filter_map(|plan| plan.strands.as_ref().map(|plan| plan.key()))
        .collect();
    assert_eq!(scatters.len(), 2);
    assert_eq!(scatters[0], scatters[1], "one scatter behind both planes");
}

#[test]
fn a_strand_field_that_reads_a_relief_is_refused_by_path() {
    // A relief is splatted *from* a layer's fields, so a field that reads one
    // is asking for itself. It is refused at the node inside the field rather
    // than at the relief, because that is the node to go and unplug.
    let graph = MaterialGraph::builder("test:cycle")
        .node("own", StrandRelief::new("blades", REPEAT))
        .node("shifted", Math::new(MathOp::Add, "own", 0.1))
        .output(PbrOutput::new().roughness("own"))
        .strands(
            "blades",
            StrandLayer::new()
                .count(8)
                .density("shifted")
                .metres(0.2, 0.03),
        )
        .into_graph();
    let material = graph
        .clone()
        .build_in(&MaterialGraphLibrary::default())
        .unwrap();
    let error = lower(&material, Target::Bake).unwrap_err();
    assert_eq!(error.path, "nodes[own]", "{error}");
    assert!(error.reason.contains("asking for itself"), "{error}");

    // And the scatter refuses it too, from the other side: the two share one
    // rule rather than each having their own.
    let refused = scatter(&StrandRequest {
        graph: &graph,
        library: &MaterialGraphLibrary::default(),
        params: &no_params(),
        layer: "blades",
        field_resolution: N,
        threads: threads(2),
    })
    .unwrap_err();
    assert!(
        format!("{refused}").contains("asking for itself"),
        "{refused}"
    );
}

#[test]
fn a_relief_that_names_no_declared_layer_is_refused_by_path() {
    let error = MaterialGraph::builder("test:missing")
        .node("relief", StrandRelief::new("stems", REPEAT))
        .output(PbrOutput::new().roughness("relief"))
        .strands("blades", StrandLayer::new().count(8))
        .build()
        .unwrap_err();
    assert_eq!(error.path, "nodes[relief].layer", "{error}");
    assert!(error.reason.contains("blades"), "{error}");
}

#[test]
fn a_blade_narrower_than_a_texel_is_a_bake_warning_by_path() {
    // The lattice check, made where it can be made: a `width_metres` is a
    // length and a resolution is a count, and only a bake holds both. The same
    // graph is fine at one resolution and undrawable at another, so this is a
    // warning about the *bake* rather than about the graph — a hair over a
    // metre-wide repeat wants two thousand texels, and says so.
    let graph = MaterialGraph::builder("test:hair")
        .node("relief", StrandRelief::new("fur", REPEAT))
        .output(PbrOutput::new().roughness("relief"))
        .strands(
            "fur",
            StrandLayer::new().count(64).metres(0.05, 0.001).lean(0.7),
        )
        .into_graph();
    let bake = |resolution: u32| {
        rasterise(&BakeRequest {
            graph: &graph,
            library: &MaterialGraphLibrary::default(),
            params: &no_params(),
            resolution,
            mips: false,
            threads: threads(2),
        })
        .unwrap()
        .1
        .warnings
    };
    // A texel at 2048 over a one-metre repeat is half a millimetre and the
    // fibre is one; at 256 the texel is four of them.
    assert!(bake(2048).is_empty(), "two texels a fibre is drawable");
    let warned = bake(256);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].path, "nodes[relief]");
    assert!(warned[0].message.contains("fur"), "{}", warned[0]);
}

#[test]
fn a_relief_splats_the_bytes_it_splatted_when_it_landed() {
    // The golden, and the same kind of pin the filter digests in `planes.rs`
    // are: a splat is `f32` arithmetic in a fixed order over a fixed set, and a
    // change to either is a change to every shipped map. Every output is here,
    // because they share a stamp and differ in what they reduce.
    //
    // `coverage` and `height` carry the bytes they carried when the node
    // landed. `along`, `id` and `color` do not, and moved once, deliberately:
    // phase 3b weights them by each contribution's own coverage instead of
    // reading them off whichever contribution stood highest, because the
    // highest point of a tapered blade is its narrowest and a hairline tip was
    // overruling a full-width root.
    let cases: [(&str, StrandReliefOutput, u64); 7] = [
        (
            "coverage",
            StrandReliefOutput::Coverage,
            0xc729_8b90_c704_7cbc,
        ),
        ("height", StrandReliefOutput::Height, 0x06bd_a16e_7216_2e51),
        ("along", StrandReliefOutput::Along, 0x5746_516b_5af3_1416),
        ("id", StrandReliefOutput::Id, 0xa9aa_4cfc_92fb_a4e2),
        ("color", StrandReliefOutput::Color, 0xb581_0c8b_811d_1ff9),
        ("mass", StrandReliefOutput::Mass, 0xa14d_1ddc_cd72_8964),
        (
            "occlusion",
            StrandReliefOutput::Occlusion,
            0x8b00_741b_7e55_2857,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, output, expected) in cases {
        let graph = lawn(output, "patches");
        let material = graph.build_in(&MaterialGraphLibrary::default()).unwrap();
        let ir = lower(&material, Target::Bake).unwrap();
        let planes: Vec<Plane> =
            rasterise_buffers(&ir, N, threads(2), &mut BakeCache::default()).unwrap();
        let found = digest(planes.last().unwrap().lanes());
        if found != expected {
            wrong.push(format!("{name}: {found:#018x} against {expected:#018x}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
