//! Default-library contracts: definitions agree with graphs and controls act
//! on the physical feature they name.
#![allow(clippy::unwrap_used, reason = "test fixtures")]
#![allow(
    clippy::cast_precision_loss,
    reason = "texel counts at a 512 bake, exact in an f32"
)]
#![allow(
    clippy::float_cmp,
    reason = "a weathering mask that did not fire leaves the substrate bit for bit: \
              every change is a Mix by it, and a margin would pass one that moved"
)]
use ashlar_material::{
    MaterialGraph, ParamValue,
    bake::{BakeRequest, Planes, rasterise},
    stdlib,
};
use std::{collections::BTreeMap, num::NonZeroUsize};

fn bake(graph: &MaterialGraph, overrides: &[(&str, f32)]) -> Planes {
    let library = stdlib::graphs();
    let params = overrides
        .iter()
        .map(|(k, v)| ((*k).into(), ParamValue::Float(*v)))
        .collect();
    rasterise(&BakeRequest {
        graph,
        library: &library,
        params: &params,
        resolution: 512,
        mips: false,
        threads: NonZeroUsize::new(4),
    })
    .unwrap()
    .0
}

#[test]
fn default_definitions_bind_existing_graphs_at_their_authored_scale() {
    let graphs = stdlib::graphs();
    let definitions = stdlib::materials();
    let finished = graphs
        .graphs
        .keys()
        .filter(|k| k.starts_with("library:"))
        .count();
    assert_eq!(definitions.materials.len(), finished);
    assert!(finished > 0);
    for (key, definition) in &definitions.materials {
        definition.check(key).unwrap();
        let graph = graphs.get(key).unwrap();
        assert_eq!(Some(definition.tile_metres), graph.tile_metres);
        assert_eq!(definition.surface.graph(), Some(key.as_str()));
        assert_eq!(
            definition.surface.params().unwrap(),
            &graph
                .params
                .iter()
                .map(|p| (p.name.clone(), p.value))
                .collect::<BTreeMap<_, _>>()
        );
    }
}

#[test]
fn cement_pores_only_remove_height_and_can_be_disabled() {
    let graph = stdlib::cast_cement();
    let off = bake(&graph, &[("pores", 0.0)]);
    let on = bake(&graph, &[("pores", 1.0)]);
    let flat = bake(&graph, &[("pores", 1.0), ("pore_depth", 0.0)]);
    let off_height = off.height.unwrap();
    assert_eq!(off_height, flat.height.unwrap());
    let on_height = on.height.unwrap();
    assert!(on_height.iter().zip(&off_height).all(|(a, b)| a <= b));
    assert!(
        on_height
            .iter()
            .zip(&off_height)
            .any(|(a, b)| a < &(b - 0.01))
    );
}

#[test]
fn concrete_formwork_controls_only_recess_the_cast_surface() {
    let graph = stdlib::formed_concrete();
    let off = bake(&graph, &[("seams", 0.0), ("ties", 0.0)]);
    let seams = bake(&graph, &[("seams", 1.0), ("ties", 0.0)]);
    let ties = bake(&graph, &[("seams", 0.0), ("ties", 1.0)]);
    let base = off.height.unwrap();
    for changed in [seams, ties] {
        let height = changed.height.unwrap();
        assert!(height.iter().zip(&base).all(|(a, b)| a <= b));
        assert!(height.iter().zip(&base).any(|(a, b)| a < &(b - 0.01)));
        // A formwork recess is local, not a height offset over the whole wall.
        assert!(
            height
                .iter()
                .zip(&base)
                .filter(|(a, b)| a.to_bits() == b.to_bits())
                .count()
                > base.len() * 9 / 10
        );
    }
}

#[test]
fn slab_identity_and_edges_close_at_both_seams_for_each_bond() {
    use ashlar_material::{
        PbrOutput, SurfaceOutput,
        interp::{Inputs, Interpreter},
        ir::{Target, lower},
        nodes::Subgraph,
    };
    let library = stdlib::graphs();
    for (columns, rows, offset) in [(4, 4, 0.0), (2, 8, 0.5), (3, 5, 0.3)] {
        for field in ["unit_id", "mask", "edge_distance", "joint_depth"] {
            let graph = MaterialGraph::builder("test:slab-seam")
                .node(
                    "field",
                    Subgraph::new("layouts:slab-lattice")
                        .param("columns", ParamValue::Int(columns))
                        .param("rows", ParamValue::Int(rows))
                        .param("offset", ParamValue::Float(offset))
                        .output(SurfaceOutput::Extra(field.into())),
                )
                .output(PbrOutput::new().roughness("field"))
                .build_in(&library)
                .unwrap();
            let ir = lower(&graph, Target::Bake).unwrap();
            let root = ir.root("roughness").unwrap();
            let interpreter = Interpreter::new(&ir);
            let sample = |uv| {
                interpreter
                    .eval_float(uv, &Inputs::default(), root)
                    .unwrap()
            };
            for along in [0.03, 0.17, 0.43, 0.79, 0.97] {
                for (a, b) in [([0.0, along], [1.0, along]), ([along, 0.0], [along, 1.0])] {
                    assert!(
                        (sample(a) - sample(b)).abs() < 0.0001,
                        "{field} fails at {a:?}/{b:?} for {columns}x{rows}, bond {offset}"
                    );
                }
            }
        }
    }
}

#[test]
fn paving_moss_changes_the_joints_and_preserves_slab_faces() {
    let graph = stdlib::paving_slabs();
    let bare = bake(&graph, &[("moss", 0.0)]);
    let grown = bake(&graph, &[("moss", 1.0)]);
    let mut probe = graph;
    probe.output.roughness = ashlar_material::Input::node("layout.mask");
    let mask = bake(&probe, &[]).roughness;
    let bare_height = bare.height.unwrap();
    let grown_height = grown.height.unwrap();
    let mut changed = 0;
    for (index, mask) in mask.iter().enumerate() {
        if *mask > 0.99999 {
            assert_eq!(
                bare.base_color[index].map(f32::to_bits),
                grown.base_color[index].map(f32::to_bits)
            );
            assert_eq!(bare_height[index].to_bits(), grown_height[index].to_bits());
        } else if grown_height[index] > bare_height[index] + 0.001 {
            changed += 1;
        }
    }
    assert!(changed > 0, "the moss control grew nothing in the joints");
}

#[test]
fn limestone_pitting_only_removes_material() {
    let graph = stdlib::cut_limestone();
    let off = bake(&graph, &[("pitting", 0.0)]).height.unwrap();
    let on = bake(&graph, &[("pitting", 1.0)]).height.unwrap();
    assert!(on.iter().zip(&off).all(|(a, b)| a <= b));
    assert!(on.iter().zip(&off).any(|(a, b)| a < &(b - 0.005)));
}

#[test]
fn dressed_margins_follow_the_block_edges_and_leave_the_face_rough() {
    let mut graph = stdlib::ashlar_blocks();
    graph.output.roughness = ashlar_material::Input::node("margin");
    graph.output.metallic = ashlar_material::Input::node("layout.edge_distance");
    let planes = bake(&graph, &[]);
    let mut dressed = 0;
    let mut face = 0;
    for (&margin, &distance) in planes.roughness.iter().zip(&planes.metallic) {
        if margin > 0.01 {
            assert!(distance < 0.011, "dressed a patch in the middle of a block");
            dressed += 1;
        } else if distance > 0.02 {
            face += 1;
        }
    }
    assert!(dressed > 0);
    assert!(
        face > planes.roughness.len() / 2,
        "the dressed margin swallowed the face"
    );
}

#[test]
fn smoothing_plaster_reduces_grain_and_roughness() {
    let graph = stdlib::plaster();
    let rough = bake(&graph, &[("smoothness", 0.0)]);
    let smooth = bake(&graph, &[("smoothness", 1.0)]);
    let slope = |height: &[f32]| height.windows(2).map(|p| (p[1] - p[0]).abs()).sum::<f32>();
    assert!(slope(smooth.height.as_ref().unwrap()) < slope(rough.height.as_ref().unwrap()) * 0.2);
    assert!(smooth.roughness.iter().sum::<f32>() < rough.roughness.iter().sum::<f32>());
}

#[test]
fn complete_plaster_loss_recovers_the_reference_brick_channels() {
    let brick = bake(&stdlib::brick(17), &[]);
    let lost = bake(&stdlib::damaged_plaster(), &[("loss", 1.0)]);
    assert_eq!(brick.base_color, lost.base_color);
    assert_eq!(brick.roughness, lost.roughness);
    assert_eq!(brick.height, lost.height);
    assert_eq!(brick.occlusion, lost.occlusion);
    let intact = bake(&stdlib::damaged_plaster(), &[("loss", 0.0)]);
    assert!(
        intact
            .height
            .unwrap()
            .iter()
            .zip(brick.height.unwrap())
            .all(|(coated, bare)| *coated > bare)
    );
}

#[test]
fn paint_loss_recovers_the_substrate_without_making_dielectrics_metallic() {
    use ashlar_material::{PbrOutput, SurfaceOutput, nodes::Subgraph};
    for metallic in [0.0, 1.0] {
        let graph = MaterialGraph::builder("test:paint-loss")
            .layer(
                "film",
                Subgraph::new("weathering:paint_film")
                    .input("loss_mask", 1.0)
                    .input("metallic", metallic)
                    .input("height", 0.4)
                    .input("roughness", 0.6)
                    .input("base_color", [0.2, 0.3, 0.4]),
                &[
                    SurfaceOutput::BaseColor,
                    SurfaceOutput::Roughness,
                    SurfaceOutput::Metallic,
                    SurfaceOutput::Height,
                ],
            )
            .output(
                PbrOutput::new()
                    .base_color("film.base_color")
                    .roughness("film.roughness")
                    .metallic("film.metallic")
                    .height("film.height"),
            )
            .into_graph();
        let planes = bake(&graph, &[]);
        assert!(
            planes
                .base_color
                .iter()
                .all(|p| p.map(f32::to_bits) == [0.2_f32, 0.3, 0.4].map(f32::to_bits))
        );
        assert!(
            planes
                .roughness
                .iter()
                .all(|p| p.to_bits() == 0.6_f32.to_bits())
        );
        assert!(
            planes
                .metallic
                .iter()
                .all(|p| p.to_bits() == metallic.to_bits())
        );
        assert!(
            planes
                .height
                .unwrap()
                .iter()
                .all(|p| p.to_bits() == 0.4_f32.to_bits())
        );
    }
}

#[test]
fn zero_age_recovers_bare_steel_and_age_adds_oxide() {
    let bare = bake(&stdlib::steel(), &[]);
    let clean = bake(&stdlib::rusted_steel(), &[("age", 0.0)]);
    assert_eq!(bare.base_color, clean.base_color);
    assert_eq!(bare.roughness, clean.roughness);
    assert_eq!(bare.metallic, clean.metallic);
    assert_eq!(bare.height, clean.height);
    let old = bake(&stdlib::rusted_steel(), &[("age", 1.0)]);
    assert!(old.metallic.iter().sum::<f32>() < clean.metallic.iter().sum::<f32>() * 0.6);
}

#[test]
fn aging_corrugated_sheet_preserves_its_profile_and_zero_age_has_no_rust_seed() {
    let mut seed_graph = stdlib::corrugated_steel();
    seed_graph.output.roughness = ashlar_material::Input::from("seed");
    let clean = bake(&seed_graph, &[("age", 0.0)]);
    assert!(clean.roughness.iter().all(|v| v.abs() < f32::EPSILON));
    let aged = bake(&stdlib::corrugated_steel(), &[("age", 1.0)]);
    assert!(aged.roughness.iter().sum::<f32>() > 0.7 * 512.0 * 512.0);
    let clean_height = clean.height.unwrap();
    let aged_height = aged.height.unwrap();
    assert!(
        aged_height
            .iter()
            .zip(&clean_height)
            .all(|(a, b)| (a - b).abs() < 0.04)
    );
}

#[test]
fn painted_boards_remain_dielectric_and_expose_timber_when_aged() {
    let graph = stdlib::painted_boards();
    let clean = bake(&graph, &[("age", 0.0)]);
    let bare = bake(&graph, &[("age", 1.0)]);
    assert!(
        clean
            .metallic
            .iter()
            .chain(&bare.metallic)
            .all(|v| v.abs() < f32::EPSILON)
    );
    let changed = clean
        .base_color
        .iter()
        .zip(&bare.base_color)
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| (x - y).abs() > 1e-6))
        .count();
    assert!(changed > clean.base_color.len() * 9 / 10);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "the grout is the same computation, bit for bit"
)]
fn glazing_smooths_ceramic_faces_without_changing_grout() {
    let graph = stdlib::ceramic_tile();
    let clay = bake(&graph, &[("glaze", 0.0)]);
    let glazed = bake(&graph, &[("glaze", 1.0)]);
    assert!(
        glazed
            .roughness
            .iter()
            .zip(&clay.roughness)
            .all(|(a, b)| a <= b)
    );
    assert!(glazed.roughness.iter().sum::<f32>() < clay.roughness.iter().sum::<f32>() * 0.4);
    assert!(
        glazed
            .base_color
            .iter()
            .zip(&clay.base_color)
            .any(|(a, b)| a == b)
    );
}

#[test]
fn road_cracks_cut_height_and_do_not_raise_the_surface() {
    let graph = stdlib::asphalt();
    let intact = bake(&graph, &[("cracking", 0.0)]).height.unwrap();
    let cracked = bake(&graph, &[("cracking", 1.0)]).height.unwrap();
    assert!(cracked.iter().zip(&intact).all(|(a, b)| a <= b));
    assert!(cracked.iter().zip(&intact).any(|(a, b)| a < &(b - 0.005)));
}

/// Mean of a float plane over the rows whose `v` lies in `from..to`.
fn band_mean(plane: &[f32], resolution: u32, from: f32, to: f32) -> f32 {
    let rows = (0..resolution).filter(|&row| {
        let v = (row as f32 + 0.5) / resolution as f32;
        v >= from && v < to
    });
    let (mut sum, mut count) = (0.0, 0);
    for row in rows {
        let start = (row * resolution) as usize;
        sum += plane[start..start + resolution as usize]
            .iter()
            .sum::<f32>();
        count += resolution;
    }
    sum / count as f32
}

fn luminance(color: &[[f32; 3]]) -> Vec<f32> {
    color
        .iter()
        .map(|[r, g, b]| 0.2126 * r + 0.7152 * g + 0.0722 * b)
        .collect()
}

#[test]
fn rain_darkens_and_smooths_the_ground_and_pools_only_where_it_is_low() {
    for graph in [stdlib::asphalt(), stdlib::road(), stdlib::paving_slabs()] {
        let dry = bake(&graph, &[]);
        let wet = bake(&graph, &[("wet", 1.0)]);
        let key = &graph.id;
        assert_eq!(
            dry.base_color,
            bake(&graph, &[("wet", 0.0)]).base_color,
            "{key}: no rain is the dry surface"
        );
        assert!(
            wet.base_color
                .iter()
                .zip(&dry.base_color)
                .all(|(w, d)| (0..3).all(|c| w[c] <= d[c])),
            "{key}: rain only darkens"
        );
        assert!(
            wet.roughness
                .iter()
                .zip(&dry.roughness)
                .all(|(w, d)| w <= d),
            "{key}: rain only smooths"
        );
        let (wet_height, dry_height) = (wet.height.unwrap(), dry.height.unwrap());
        assert!(
            wet_height.iter().zip(&dry_height).all(|(w, d)| w >= d),
            "{key}: a puddle fills a hollow and cuts nothing"
        );
        let pooled = wet.roughness.iter().filter(|r| **r < 0.1).count();
        let share = pooled as f32 / wet.roughness.len() as f32;
        assert!(
            (0.03..0.4).contains(&share),
            "{key}: puddles cover {share} of the surface"
        );
    }
}

#[test]
fn stained_concrete_streaks_run_down_from_the_slab_edge_and_nowhere_else() {
    let graph = stdlib::stained_concrete();
    let clean = bake(&graph, &[("streaks", 0.0), ("grime", 0.0)]);
    let streaked = bake(&graph, &[("streaks", 1.0), ("grime", 0.0)]);
    let (clean_l, streaked_l) = (
        luminance(&clean.base_color),
        luminance(&streaked.base_color),
    );
    let darkening: Vec<f32> = clean_l
        .iter()
        .zip(&streaked_l)
        .map(|(c, s)| c - s)
        .collect();
    assert!(darkening.iter().all(|d| *d >= 0.0), "soot only darkens");
    // `v` is height up the storey: the slab edge is at the top, and the run
    // reaches six tenths of the storey down from it and no further.
    assert!(band_mean(&darkening, 512, 0.6, 0.9) > 3.0 * band_mean(&darkening, 512, 0.35, 0.45));
    // The slab edge's band is grown by a step before the walk, and a step
    // over the top of the repeat is the first rows of the storey above it,
    // so the storey line itself is left out.
    let lower = (512 * 30..512 * 150).all(|i| clean.base_color[i] == streaked.base_color[i]);
    assert!(lower, "the lower storey is exactly the clean concrete");
    assert_eq!(clean.height, streaked.height, "soot is no relief");
}

#[test]
fn stained_concrete_grime_sits_at_the_foot_of_the_storey() {
    let graph = stdlib::stained_concrete();
    let clean = bake(&graph, &[("streaks", 0.0), ("grime", 0.0)]);
    let grimy = bake(&graph, &[("streaks", 0.0), ("grime", 1.0)]);
    let (clean_l, grimy_l) = (luminance(&clean.base_color), luminance(&grimy.base_color));
    let darkening: Vec<f32> = clean_l.iter().zip(&grimy_l).map(|(c, s)| c - s).collect();
    assert!(band_mean(&darkening, 512, 0.0, 0.05) > 0.2 * band_mean(&clean_l, 512, 0.0, 0.05));
    assert!(
        (512 * 60..512 * 512).all(|i| clean.base_color[i] == grimy.base_color[i]),
        "nothing above the grime band moves"
    );
}

#[test]
fn an_office_window_is_lit_towards_its_ceiling_and_blinds_only_dim_it() {
    let graph = stdlib::office_window();
    let open = bake(&graph, &[("blinds", 0.0)]);
    let shut = bake(&graph, &[("blinds", 1.0)]);
    let glow = luminance(&open.emissive.unwrap());
    assert!(band_mean(&glow, 512, 0.65, 0.85) > 1.5 * band_mean(&glow, 512, 0.12, 0.3));
    let dimmed = luminance(&shut.emissive.unwrap());
    assert!(dimmed.iter().zip(&glow).all(|(s, o)| s <= o));
    assert!(dimmed.iter().zip(&glow).any(|(s, o)| s < &(o - 0.1)));
}
