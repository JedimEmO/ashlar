//! The city surfaces: each control acts on the feature it names, and each
//! layout lands where the city kit's grid puts it.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test fixtures; the exact comparisons are against values a Mix or a clamp writes exactly"
)]
use ashlar_material::{
    MaterialGraph, ParamValue,
    bake::{BakeRequest, Planes, rasterise},
    stdlib,
};
use std::num::NonZeroUsize;

const RESOLUTION: u32 = 512;

fn bake(graph: &MaterialGraph, overrides: &[(&str, ParamValue)]) -> Planes {
    let library = stdlib::graphs();
    let params = overrides.iter().map(|(k, v)| ((*k).into(), *v)).collect();
    rasterise(&BakeRequest {
        graph,
        library: &library,
        params: &params,
        resolution: RESOLUTION,
        mips: false,
        threads: NonZeroUsize::new(4),
    })
    .unwrap()
    .0
}

fn float(value: f32) -> ParamValue {
    ParamValue::Float(value)
}

fn luminance(color: &[f32; 3]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

/// The texel under a point `x` metres across and `y` metres along (or up) a
/// repeat of `tile` metres.
fn texel(tile: [f32; 2], x: f32, y: f32) -> usize {
    let column = ((x / tile[0]) * RESOLUTION as f32) as usize;
    let row = ((y / tile[1]) * RESOLUTION as f32) as usize;
    row.min(RESOLUTION as usize - 1) * RESOLUTION as usize + column.min(RESOLUTION as usize - 1)
}

fn lit_share(planes: &Planes) -> f32 {
    let emissive = planes.emissive.as_ref().unwrap();
    emissive.iter().filter(|e| luminance(e) > 0.05).count() as f32 / emissive.len() as f32
}

#[test]
fn curtain_wall_lights_the_share_of_offices_lit_asks_for() {
    let graph = stdlib::curtain_wall();
    let dark = bake(&graph, &[("lit", float(0.0))]);
    let some = bake(&graph, &[]);
    let all = bake(&graph, &[("lit", float(1.0))]);
    assert_eq!(lit_share(&dark), 0.0);
    // Glass is about two thirds of the facade; all of it lit, then a third.
    let full = lit_share(&all);
    assert!(full > 0.55, "{full}");
    let share = lit_share(&some) / full;
    assert!((0.18..0.5).contains(&share), "{share}");
    // Lighting changes the emission and nothing the sun sees.
    assert_eq!(dark.base_color, all.base_color);
    assert_eq!(dark.height, all.height);
}

#[test]
fn curtain_wall_frames_stand_proud_on_the_storey_and_bay_grid() {
    let planes = bake(&stdlib::curtain_wall(), &[]);
    let height = planes.height.unwrap();
    let tile = [16.0, 15.2];
    // A bay mullion at 4 m and a slab transom at the storey line stand over
    // the vision glass in the middle of a pane.
    let glass = height[texel(tile, 4.5, 2.0)];
    assert!(height[texel(tile, 4.0, 2.0)] > glass + 0.4);
    assert!(height[texel(tile, 4.5, 3.8)] > glass + 0.4);
    // The spandrel over the slab is opaque, and a shade lighter than glass.
    let spandrel = planes.base_color[texel(tile, 4.5, 0.45)];
    assert!(luminance(&spandrel) > luminance(&planes.base_color[texel(tile, 4.5, 2.0)]) * 0.9);
}

#[test]
fn curtain_wall_tint_colours_the_glass_and_not_the_frame() {
    let graph = stdlib::curtain_wall();
    let clear = bake(&graph, &[]);
    let bronze = bake(&graph, &[("tint", ParamValue::Color([1.2, 0.8, 0.5]))]);
    let tile = [16.0, 15.2];
    let frame = texel(tile, 4.0, 2.0);
    assert_eq!(clear.base_color[frame], bronze.base_color[frame]);
    // Over blue-grey glass a bronze tint shifts the balance to red.
    let glass = texel(tile, 4.5, 2.0);
    let balance = |c: [f32; 3]| c[0] / c[2];
    assert!(balance(bronze.base_color[glass]) > balance(clear.base_color[glass]) * 2.0);
}

#[test]
fn window_band_sets_its_window_behind_a_projecting_sill() {
    let graph = stdlib::window_band();
    let planes = bake(&graph, &[]);
    let height = planes.height.unwrap();
    let tile = [16.0, 15.2];
    let panel = height[texel(tile, 2.5, 0.6)];
    let sill = height[texel(tile, 2.5, 1.09)];
    let glass = height[texel(tile, 2.5, 2.0)];
    assert!(sill > panel, "{sill} {panel}");
    assert!(glass < panel - 0.5, "{glass} {panel}");
    // `color` is the cladding's, and `lit` the windows'.
    let red = bake(&graph, &[("color", ParamValue::Color([0.5, 0.1, 0.1]))]);
    let at = texel(tile, 2.5, 0.6);
    assert!(red.base_color[at][0] > red.base_color[at][1] * 3.0);
    assert_eq!(
        bake(&graph, &[("lit", float(0.0))])
            .emissive
            .map(|e| lit_share_of(&e)),
        Some(0.0)
    );
}

fn lit_share_of(emissive: &[[f32; 3]]) -> f32 {
    emissive.iter().filter(|e| luminance(e) > 0.05).count() as f32 / emissive.len() as f32
}

#[test]
fn road_markings_sit_on_the_lines_a_road_kit_expects() {
    let graph = stdlib::road();
    let fresh = bake(&graph, &[("wear", float(0.0))]);
    let tile = [12.0, 12.0];
    let lane = luminance(&fresh.base_color[texel(tile, 3.0, 1.5)]);
    // Edge lines 0.6 m in from each side, the centre line dashed 3 m on, 3 m
    // off from the start of the repeat.
    for (x, y) in [(0.6, 5.0), (11.4, 5.0), (6.0, 1.5), (6.0, 7.5)] {
        let paint = luminance(&fresh.base_color[texel(tile, x, y)]);
        assert!(paint > lane * 3.0, "({x}, {y}): {paint} against {lane}");
    }
    let gap = luminance(&fresh.base_color[texel(tile, 6.0, 4.5)]);
    assert!(gap < lane * 2.0, "{gap} against {lane}");
    // Wear takes paint away and puts none down.
    let worn = bake(&graph, &[("wear", float(1.0))]);
    let painted = |planes: &Planes| {
        planes
            .base_color
            .iter()
            .filter(|c| luminance(c) > 0.3)
            .count()
    };
    assert!(
        painted(&worn) * 2 < painted(&fresh),
        "{} {}",
        painted(&worn),
        painted(&fresh)
    );
}

#[test]
fn interior_panelling_recesses_its_seams_and_lights_only_its_strips() {
    let planes = bake(&stdlib::interior_panelling(), &[]);
    let height = planes.height.unwrap();
    let tile = [4.0, 3.0];
    // A seam between two panels sits below the faces either side of it.
    let seam = height[texel(tile, 1.0, 0.6)];
    let face = height[texel(tile, 0.5, 0.6)];
    assert!(face > seam + 0.2, "{face} {seam}");
    // Emission is confined to strips, coves and screens: a small share.
    let emissive = planes.emissive.unwrap();
    let share = lit_share_of(&emissive);
    assert!((0.002..0.06).contains(&share), "{share}");
}

#[test]
fn holo_sign_glows_in_its_own_colour() {
    let graph = stdlib::holo_sign();
    let cyan = bake(&graph, &[]).emissive.unwrap();
    let amber = bake(&graph, &[("color", ParamValue::Color([1.0, 0.55, 0.05]))])
        .emissive
        .unwrap();
    let total = |e: &[[f32; 3]], channel: usize| e.iter().map(|c| c[channel]).sum::<f32>();
    assert!(total(&cyan, 2) > total(&cyan, 0));
    assert!(total(&amber, 0) > total(&amber, 2));
    assert!(lit_share_of(&cyan) > 0.05, "{}", lit_share_of(&cyan));
}

#[test]
fn every_emitting_city_surface_carries_a_radiance() {
    let materials = stdlib::materials();
    for key in [
        "library:curtain-wall",
        "library:window-band",
        "library:interior-panelling",
        "library:holo-sign",
    ] {
        assert!(
            materials.materials[key].emissive.iter().all(|c| *c > 0.0),
            "{key} emits nothing"
        );
    }
    assert_eq!(materials.materials["library:road"].emissive, [0.0; 3]);
}
