//! Behavioural probes for the library's reference materials: the brick and
//! cobblestone rebuilt against scans, their moss variants, the grass and the
//! moss carpet.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "deterministic test fixtures"
)]
use ashlar_material::stdlib;
use ashlar_material::{
    MaterialGraph, ParamValue,
    bake::{BakeRequest, Planes, plan, rasterise},
    ir::{IrType, ValueId},
    memo::{MEMO_BUDGET, MemoPlan, stages},
};

/// The resolution the references were authored and reviewed at.
const REFERENCE_RESOLUTION: u32 = 2048;
use std::num::NonZeroUsize;

fn sample(graph: &MaterialGraph, values: &[(&str, f32)]) -> Planes {
    let params = values
        .iter()
        .map(|(k, v)| (k.to_string(), ParamValue::Float(*v)))
        .collect();
    rasterise(&BakeRequest {
        graph,
        library: &stdlib::graphs(),
        params: &params,
        resolution: 512,
        mips: false,
        threads: NonZeroUsize::new(4),
    })
    .unwrap()
    .0
}
fn mask(graph: &MaterialGraph, name: &str, values: &[(&str, f32)]) -> Vec<f32> {
    let mut graph = graph.clone();
    graph.output.base_color = graph.output.extra[name].clone();
    sample(&graph, values)
        .base_color
        .iter()
        .map(|c| c[0])
        .collect()
}
#[test]
fn mortar_rises_and_erosion_preserves_brick_identity() {
    let graph = stdlib::brick(17);
    let low = sample(&graph, &[("mortar_fill", 0.1)]);
    let high = sample(&graph, &[("mortar_fill", 0.5)]);
    let a = mask(&graph, "brick_mask", &[("mortar_fill", 0.1)]);
    let b = mask(&graph, "brick_mask", &[("mortar_fill", 0.5)]);
    assert!(a.iter().zip(&b).all(|(a, b)| a + 1e-6 >= *b));
    assert!(high.height.unwrap().iter().sum::<f32>() > low.height.unwrap().iter().sum::<f32>());
    assert_eq!(
        mask(&graph, "brick_id", &[]),
        mask(&graph, "brick_id", &[("erosion_mm", 0.0)])
    );
    let zero = sample(&graph, &[("erosion_mm", 0.0)]);
    assert_ne!(zero.height, sample(&graph, &[]).height);
    let mortar = mask(&graph, "mortar_mask", &[]);
    let brick = mask(&graph, "brick_mask", &[]);
    assert!(
        mortar
            .iter()
            .zip(brick)
            .all(|(a, b)| (a + b - 1.0).abs() < 1e-6)
    );
}
#[test]
fn brick_lime_is_visible_and_its_control_increases_coverage() {
    let graph = stdlib::brick(17);
    let bare = mask(&graph, "lime_residue", &[("lime_residue", 0.0)]);
    let normal = mask(&graph, "lime_residue", &[]);
    let heavy = mask(&graph, "lime_residue", &[("lime_residue", 0.8)]);
    assert!(bare.iter().all(|v| *v == 0.0));
    let coverage = normal.iter().map(|v| f64::from(*v)).sum::<f64>()
        / f64::from(u32::try_from(normal.len()).unwrap());
    assert!(
        (0.01..0.20).contains(&coverage),
        "lime coverage: {coverage}"
    );
    assert!(normal.iter().zip(&heavy).all(|(a, b)| a <= b));
    assert!(heavy.iter().sum::<f32>() > normal.iter().sum::<f32>());
    assert_eq!(
        mask(&graph, "brick_id", &[("lime_residue", 0.0)]),
        mask(&graph, "brick_id", &[("lime_residue", 0.8)])
    );
}

#[test]
fn missing_bricks_remove_relief_without_moving_the_bond() {
    let graph = stdlib::brick(17);
    let intact = sample(&graph, &[("missing_bricks", 0.0)]);
    let damaged = sample(&graph, &[]);
    let intact_height = intact.height.unwrap();
    let damaged_height = damaged.height.unwrap();
    assert!(
        intact_height
            .iter()
            .zip(&damaged_height)
            .all(|(a, b)| a >= b)
    );
    assert!(
        intact_height
            .iter()
            .zip(&damaged_height)
            .any(|(a, b)| a - b > 0.1)
    );
    assert!(
        mask(&graph, "hole_mask", &[("missing_bricks", 0.0)])
            .iter()
            .all(|v| *v == 0.0)
    );
    assert_eq!(
        mask(&graph, "brick_id", &[("missing_bricks", 0.0)]),
        mask(&graph, "brick_id", &[])
    );
}

#[test]
fn reference_seeds_validate() {
    for seed in [17, 29, 43] {
        let mut library = stdlib::graphs();
        for graph in [
            stdlib::brick(seed),
            stdlib::cobblestone(seed),
            stdlib::grass(seed),
            stdlib::moss_carpet(seed),
        ] {
            library.insert(graph);
        }
        library.check().unwrap();
    }
}

#[test]
fn soi_grout_owns_color_and_height_and_damage_only_removes_stone() {
    let graph = stdlib::cobblestone(17);
    let low = sample(&graph, &[("grout_height", 0.15)]);
    let high = sample(&graph, &[("grout_height", 0.7)]);
    assert!(
        low.height
            .as_ref()
            .unwrap()
            .iter()
            .zip(high.height.as_ref().unwrap())
            .all(|(a, b)| a <= b)
    );
    let low_mask = mask(&graph, "stone_mask", &[("grout_height", 0.15)]);
    let high_mask = mask(&graph, "stone_mask", &[("grout_height", 0.7)]);
    assert!(low_mask.iter().zip(&high_mask).all(|(a, b)| a + 1e-6 >= *b));
    assert!(
        low_mask
            .iter()
            .zip(&high_mask)
            .filter(|(a, b)| **a - **b > 0.5)
            .count()
            > 1000
    );
    let clean = mask(&graph, "damage", &[("edge_damage", 0.0)]);
    let damaged = mask(&graph, "damage", &[("edge_damage", 1.0)]);
    assert!(clean.iter().zip(&damaged).all(|(a, b)| a + 1e-6 >= *b));
    assert!(clean.iter().zip(&damaged).any(|(a, b)| a - b > 0.05));
    // Color and roughness share the exported height ownership mask. Every
    // texel swallowed by high grout must lose its stone-specific appearance.
    assert!(
        low.base_color
            .iter()
            .zip(&high.base_color)
            .zip(low_mask.iter().zip(&high_mask))
            .filter(|(_, (a, b))| **a > 0.99 && **b < 0.01)
            .all(|((a, b), _)| a != b)
    );
}

/// Why a bake is fused: the stages of a bake — every plane, then the outputs
/// — evaluate one expression between them rather than one closure each.
///
/// The SOI cobblestone is the graph that made the case. When memoisation was
/// written (2026-09-18, nineteen planes then) its stages' closures added up to
/// 13,932 instructions per texel against an expression of 4,196, because the height its occlusion plane rasterises is rasterised again by
/// the curvature plane, again by the `height` output and again inside the
/// planes and outputs above them.
#[test]
fn the_soi_stages_evaluate_one_expression_between_them() {
    let library = stdlib::graphs();
    let graph = library.get("library:soi-cobblestone").unwrap();
    let resolution = REFERENCE_RESOLUTION;
    let params = std::collections::BTreeMap::new();
    let ir = plan(&BakeRequest {
        graph,
        library: &library,
        params: &params,
        resolution,
        mips: true,
        threads: NonZeroUsize::new(8),
    })
    .unwrap()
    .ir;

    let stages = stages(&ir, resolution);
    assert_eq!(
        stages.len(),
        ir.buffers().len() + 1,
        "every plane is a stage, and so are the outputs",
    );

    let naive: usize = stages
        .iter()
        .map(|stage| {
            ir.reaches(&stage.roots)
                .into_iter()
                .filter(|live| *live)
                .count()
        })
        .sum();
    assert!(
        naive > 3 * ir.len(),
        "{naive} against {} is what there is to win",
        ir.len()
    );

    // Memoising every value it can: each instruction evaluated exactly once
    // for the whole bake, the sources that read nothing counted once because
    // there is nothing under them to keep.
    let whole = MemoPlan::analyse_with(&ir, resolution, 0);
    assert_eq!(whole.evaluations(), ir.len());
    let outputs = whole.stage(stages.len() - 1).unwrap();
    assert!(
        !outputs.reads.is_empty(),
        "the outputs read what the planes left, or the height is rasterised twice",
    );

    // And what the bake actually runs, which keeps a plane only where it fits
    // the budget: within a tenth of evaluating each instruction once, and under
    // half of what it was. The bound was a half while `MEMO_BUDGET` was
    // 256 MiB, which the face and grout rework had grown the graph past: the
    // ladder fell to a `min_reach` of 256 and the bake evaluated 10,806
    // instructions a texel against an expression of 7,785. A gibibyte buys the
    // `min_reach` 16 rung back, and this is what that is worth.
    let budgeted = MemoPlan::analyse(&ir, resolution);
    assert!(
        budgeted.evaluations() < ir.len() * 11 / 10,
        "{} per texel against {}",
        budgeted.evaluations(),
        ir.len()
    );
    assert!(budgeted.evaluations() * 2 < naive);

    let texel = (resolution as usize).pow(2) * size_of::<f32>();
    assert!(
        budgeted.peak_lanes() * texel <= MEMO_BUDGET,
        "{} lanes at {resolution} is over the budget",
        budgeted.peak_lanes()
    );

    // And what a pass that never rasterises the outputs pays. Part of this
    // table is read by the outputs and by nobody else, so a caller that drops
    // it on the way out — `rasterise_buffers`, `rasterise_wanted`, and so
    // `ashlar-bevy`'s CPU fallback — must not be writing that part. It is a
    // part rather than the whole, which is worth pinning in both directions:
    // the plane stages really do read most of what they leave each other.
    let for_planes = budgeted.peak_lanes_for_planes(&ir);
    assert!(
        for_planes < budgeted.peak_lanes(),
        "{for_planes} lanes for the planes alone against {} for the whole bake",
        budgeted.peak_lanes()
    );
    let wanted = budgeted.read_before_outputs(ir.len());
    let lanes = |value: &ValueId| ir.type_of(*value).map_or(0, IrType::components);
    let (mut written, mut unread) = (0_usize, 0_usize);
    for stage in budgeted.stages() {
        for value in &stage.writes {
            written += lanes(value);
            if wanted.get(value.index()).copied() != Some(true) {
                unread += lanes(value);
            }
        }
    }
    assert!(
        unread > 0,
        "{unread} of {written} lanes are the outputs' alone"
    );
    eprintln!(
        "memo table at {resolution}: peak {} MB whole, {} MB planes only; written {} MB of \
         which {} MB only the outputs read",
        (budgeted.peak_lanes() * texel) >> 20,
        (for_planes * texel) >> 20,
        (written * texel) >> 20,
        (unread * texel) >> 20
    );
}

#[test]
fn moss_variants_share_the_brick_and_grow_from_its_masks() {
    let base = stdlib::brick(17);
    let light = stdlib::brick_moss(false);
    let heavy = stdlib::brick_moss(true);
    let original = sample(&base, &[]);
    let off = sample(&heavy, &[("moss_amount", 0.0)]);
    assert_eq!(
        off, original,
        "zero moss must recover every base channel exactly"
    );
    assert_eq!(mask(&light, "brick_id", &[]), mask(&base, "brick_id", &[]));
    let low = mask(&light, "moss_mask", &[]);
    let high = mask(&heavy, "moss_mask", &[]);
    assert!(low.iter().zip(&high).all(|(a, b)| a <= b));
    assert!(low.iter().any(|v| *v > 0.1));
    assert!(high.iter().sum::<f32>() > low.iter().sum::<f32>() * 1.5);
    let habitat = mask(&light, "habitat", &[]);
    let mut sheltered = (0.0, 0.0);
    let mut exposed = (0.0, 0.0);
    for (growth, shelter) in low.iter().zip(habitat) {
        let group = if shelter > 0.5 {
            &mut sheltered
        } else {
            &mut exposed
        };
        group.0 += growth;
        group.1 += 1.0;
    }
    assert!(sheltered.0 / sheltered.1 > exposed.0 / exposed.1);
}

#[test]
fn cobblestone_moss_preserves_the_base_and_prefers_grout() {
    let base = stdlib::cobblestone(17);
    let light = stdlib::cobblestone_moss(false);
    let heavy = stdlib::cobblestone_moss(true);
    assert_eq!(sample(&base, &[]), sample(&heavy, &[("moss_amount", 0.0)]));
    assert_eq!(mask(&base, "stone_id", &[]), mask(&light, "stone_id", &[]));
    let low = mask(&light, "moss_mask", &[]);
    let high = mask(&heavy, "moss_mask", &[]);
    assert!(low.iter().zip(&high).all(|(a, b)| a <= b));
    assert!(high.iter().sum::<f32>() > low.iter().sum::<f32>() * 1.5);
    let habitat = mask(&light, "habitat", &[]);
    let mut grout = (0.0, 0.0);
    let mut stone = (0.0, 0.0);
    for (growth, shelter) in low.iter().zip(habitat) {
        let bucket = if shelter > 0.5 {
            &mut grout
        } else {
            &mut stone
        };
        bucket.0 += growth;
        bucket.1 += 1.0;
    }
    assert!(grout.1 > 0.0 && stone.1 > 0.0);
    assert!(grout.0 / grout.1 > stone.0 / stone.1);
    assert_eq!(REFERENCE_RESOLUTION, 2048);
}

/// The strand vocabulary is not grass-shaped: `library:moss-carpet` scatters
/// the same way over a mask rather than a field, as a tube rather than a
/// ribbon, standing up rather than lying over.
///
/// Three claims and a count. The count is the one worth having: a level of
/// detail is a rank prefix, so a caller can read the cost of a repeat off the
/// set before it has built a single triangle.
#[test]
fn the_moss_carpet_scatters_upright_fibres_off_the_shared_moss_mask() {
    use ashlar_material::{
        StrandProfile,
        strands::{StrandRequest, scatter},
    };

    let library = stdlib::graphs();
    let graph = library.get("library:moss-carpet").unwrap();
    let params = std::collections::BTreeMap::new();
    let request = StrandRequest {
        graph,
        library: &library,
        params: &params,
        layer: "shoots",
        field_resolution: ashlar_material::strands::FIELD_RESOLUTION,
        threads: NonZeroUsize::new(4),
    };
    let set = scatter(&request).unwrap();

    // A tube, which is what makes this the second profile rather than the first
    // material again, and one segment, which is what pays for it.
    assert_eq!(set.profile(), StrandProfile::Fibre);
    assert_eq!(set.segments(), 1);

    // Upright. The lawn's blades lean between a third and two thirds of a
    // quarter turn; these stand, and the mean says so rather than one strand
    // that happened to.
    let total: f64 = set.strands().iter().map(|s| f64::from(s.lean)).sum();
    let mean_lean = total / f64::from(u32::try_from(set.len()).unwrap());
    assert!(
        (0.02..0.30).contains(&mean_lean),
        "a moss shoot stands: mean lean {mean_lean}"
    );

    // Off the mask rather than off a field: the density is a *threshold*, so a
    // layer growing on a coverage mask keeps some cells and not others, and the
    // bare ground between patches is bare in the geometry as well as in the
    // texture.
    let lattice = usize::try_from(set.count()[0] * set.count()[1]).unwrap()
        * usize::try_from(stdlib::CARPET_SHOOTS_PER_CELL).unwrap();
    assert!(
        set.len() < lattice,
        "{} of {lattice} kept; a mask that keeps everything is not a mask",
        set.len()
    );
    assert!(set.len() * 4 > lattice, "{} of {lattice} kept", set.len());

    // What a full-detail repeat costs, as triangles. Six per shoot: a tube has
    // three faces and a face is two triangles.
    let triangles = set.len() * set.shape().triangles_per_strand();
    assert!(
        triangles < 800_000,
        "{triangles} triangles over one repeat is past the budget"
    );

    // And a second scatter at another thread count is the same set, which is
    // the claim every strand material rests on.
    let again = scatter(&StrandRequest {
        threads: NonZeroUsize::new(1),
        ..request
    })
    .unwrap();
    assert_eq!(set, again, "a scatter does not depend on its threads");
}

/// What one full-detail repeat of the lawn costs, and that the three layers are
/// the three things they claim to be.
///
/// The count is the one worth having, for the reason the carpet's test gives: a
/// level of detail is a rank prefix, so a caller reads the cost of a repeat off
/// the sets before it has built a triangle. The budget is 800 000 and the
/// material's own doc comment quotes the number this asserts, so the two cannot
/// drift apart without a test going red.
#[test]
fn the_grass_grows_three_layers_inside_its_triangle_budget() {
    use ashlar_material::strands::{StrandRequest, scatter};

    let library = stdlib::graphs();
    let graph = library.get("library:grass").unwrap();
    let params = std::collections::BTreeMap::new();
    let set = |layer| {
        scatter(&StrandRequest {
            graph,
            library: &library,
            params: &params,
            layer,
            field_resolution: ashlar_material::strands::FIELD_RESOLUTION,
            threads: NonZeroUsize::new(4),
        })
        .unwrap()
    };
    let blades = set("blades");
    let fibres = set("fibres");
    let stragglers = set("stragglers");

    // The mat lies over and the escapes stand up. That is the whole division of
    // labour between them: a leaning strand splats a stroke into the relief and
    // an upright one breaks the silhouette, and no one strand does both.
    let mean_lean = |s: &ashlar_material::strands::StrandSet| {
        s.strands().iter().map(|s| f64::from(s.lean)).sum::<f64>()
            / f64::from(u32::try_from(s.len()).unwrap())
    };
    assert!(
        mean_lean(&blades) > 0.30,
        "a blade lies over: mean lean {}",
        mean_lean(&blades)
    );
    assert!(
        mean_lean(&stragglers) < 0.20,
        "a straggler stands: mean lean {}",
        mean_lean(&stragglers)
    );

    // Sparse, and by a lot: an escape is meant to be countable against the mat
    // it grows out of.
    assert!(
        stragglers.len() * 8 < fibres.len(),
        "{} stragglers against {} fibres is not sparse",
        stragglers.len(),
        fibres.len()
    );

    let triangles = [&blades, &fibres, &stragglers]
        .into_iter()
        .map(|s| s.len() * s.shape().triangles_per_strand())
        .sum::<usize>();
    assert!(
        triangles < 800_000,
        "{triangles} triangles over one repeat is past the budget"
    );
    // Quoted in the module doc comment of `stdlib/reference_grass.rs`; if the scatter
    // moves, both move together.
    assert_eq!(
        triangles,
        688_818,
        "the documented repeat cost has moved: {} blades, {} fibres, {} stragglers",
        blades.len(),
        fibres.len(),
        stragglers.len()
    );
}

/// One square metre of ground mapped to exactly one repeat, which is the
/// surface a set is planted on in both halves of the test below.
fn ground() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
    (
        vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
        vec![[0.0, 1.0, 0.0]; 4],
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        vec![0, 1, 2, 0, 2, 3],
    )
}

/// Every layer one preset grows, freshly scattered.
fn scatter_preset(preset: &str) -> Vec<ashlar_material::strands::StrandSet> {
    use ashlar_material::strands::{StrandRequest, scatter};

    let library = stdlib::graphs();
    let graph = library.get(&format!("library:{preset}")).unwrap();
    let params = std::collections::BTreeMap::new();
    let settings = stdlib::materials().materials[&format!("library:{preset}")]
        .strands
        .clone()
        .unwrap();
    settings
        .layers
        .iter()
        .map(|layer| {
            scatter(&StrandRequest {
                graph,
                library: &library,
                params: &params,
                layer,
                field_resolution: ashlar_material::strands::FIELD_RESOLUTION,
                threads: NonZeroUsize::new(4),
            })
            .unwrap()
        })
        .collect()
}

#[test]
fn a_baked_set_meshes_to_the_same_triangles_as_the_scatter_it_came_from() {
    // The claim the whole file format exists to support, and the reason it is
    // lossless: a game that reads a lawn off disk draws the lawn the content
    // step drew, vertex for vertex, and not one that is nearly it. Both
    // presets, because they are the two profiles — a ribbon and a tube — and
    // the mesh builder branches on that.
    use ashlar_material::strands::{SurfaceTriangles, file, mesh, place};

    let (positions, normals, uvs, indices) = ground();
    let surface = SurfaceTriangles {
        positions: &positions,
        normals: &normals,
        uvs: &uvs,
        indices: &indices,
    };
    for preset in ["grass", "moss-carpet"] {
        let scattered = scatter_preset(preset);
        let baked = file::read(&file::write(&scattered).unwrap()).unwrap();
        assert_eq!(baked, scattered, "{preset}: the set did not round trip");
        for (baked, scattered) in baked.iter().zip(&scattered) {
            // Every level of detail, because a level is a rank prefix and a
            // prefix of a set that had been reordered would be a different
            // handful of strands rather than a different order of the same.
            for keep in [1.0_f32, 0.25, 0.0625] {
                let one = mesh(&place(baked, &surface, keep), baked.shape());
                let other = mesh(&place(scattered, &surface, keep), scattered.shape());
                assert_eq!(
                    one,
                    other,
                    "{preset}:{} at keep {keep} meshed differently off disk",
                    baked.layer()
                );
                assert!(one.triangles() > 0, "{preset} grew nothing at keep {keep}");
            }
        }
    }
}
