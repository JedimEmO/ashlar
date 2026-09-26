//! The frontier-settlement surfaces: each control acts on the feature it
//! names and nothing else.
#![allow(
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    reason = "test fixtures; the exact comparisons are against values a Mix or a clamp writes exactly"
)]
use ashlar_material::{
    MaterialGraph, ParamValue,
    bake::{BakeRequest, Planes, rasterise},
    stdlib,
};
use std::num::NonZeroUsize;

fn bake(graph: &MaterialGraph, overrides: &[(&str, ParamValue)]) -> Planes {
    let library = stdlib::graphs();
    let params = overrides.iter().map(|(k, v)| ((*k).into(), *v)).collect();
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

fn float(value: f32) -> ParamValue {
    ParamValue::Float(value)
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() / 2]
}

fn luminance(color: &[f32; 3]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

#[test]
fn hull_plating_wear_bares_metal_without_moving_the_relief() {
    let graph = stdlib::hull_plating();
    let clean = bake(&graph, &[("wear", float(0.0))]);
    let worn = bake(&graph, &[("wear", float(1.0))]);
    // No wear, no bare metal anywhere; full wear bares the arrises and the
    // fastener heads.
    assert!(clean.metallic.iter().all(|m| *m == 0.0));
    let bared = worn.metallic.iter().filter(|m| **m > 0.5).count();
    assert!(bared > worn.metallic.len() / 100, "{bared}");
    // The relief is the panels' own and does not read `wear`.
    assert_eq!(clean.height, worn.height);
}

#[test]
fn hull_plating_seams_sit_below_the_panel_faces() {
    let planes = bake(&stdlib::hull_plating(), &[]);
    let height = planes.height.unwrap();
    let face = median(&height);
    let lowest = height.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(face - lowest > 0.3, "{face} {lowest}");
    // A seam is a recess, not a line of paint: it is low, and grime makes it
    // darker than the faces round it.
    let seam = height.iter().position(|h| *h == lowest).unwrap();
    let faces = median(&planes.base_color.iter().map(luminance).collect::<Vec<_>>());
    assert!(luminance(&planes.base_color[seam]) < faces * 0.75);
}

#[test]
fn hull_plating_color_tints_the_paint() {
    let graph = stdlib::hull_plating();
    let dark = bake(&graph, &[("color", ParamValue::Color([0.1, 0.1, 0.1]))]);
    let light = bake(&graph, &[("color", ParamValue::Color([0.7, 0.7, 0.7]))]);
    let mean = |planes: &Planes| {
        planes.base_color.iter().map(luminance).sum::<f32>() / planes.base_color.len() as f32
    };
    assert!(mean(&light) > mean(&dark) * 2.0);
}

#[test]
fn tread_plate_lugs_stand_proud_and_rust_only_takes_the_metal() {
    let graph = stdlib::tread_plate();
    let bright = bake(&graph, &[("rust", float(0.0))]);
    let rusty = bake(&graph, &[("rust", float(1.0))]);
    let height = bright.height.clone().unwrap();
    let (low, high) = height
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
            (l.min(*v), h.max(*v))
        });
    assert!(high - low > 0.45, "{low}..{high}");
    // Most of the plate is the plate, not the lugs.
    assert!(median(&height) < low + (high - low) * 0.35);
    assert!(bright.metallic.iter().all(|m| *m == 1.0));
    assert!(rusty.metallic.iter().any(|m| *m < 0.1));
    assert!(
        rusty
            .metallic
            .iter()
            .zip(&bright.metallic)
            .all(|(r, b)| r <= b)
    );
    assert_eq!(bright.height, rusty.height);
}

#[test]
fn adobe_color_is_the_clay_and_the_relief_is_hand_worked() {
    let graph = stdlib::adobe();
    let pale = bake(&graph, &[("color", ParamValue::Color([0.6, 0.5, 0.4]))]);
    let dark = bake(&graph, &[("color", ParamValue::Color([0.2, 0.15, 0.1]))]);
    assert_eq!(pale.height, dark.height);
    let height = pale.height.unwrap();
    let (low, high) = height
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
            (l.min(*v), h.max(*v))
        });
    assert!((0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high));
    assert!(high - low > 0.25, "{low}..{high}");
    assert!(pale.roughness.iter().all(|r| *r > 0.7));
}

#[test]
fn desert_sand_is_matt_rippled_ground() {
    let planes = bake(&stdlib::desert_sand(), &[]);
    let height = planes.height.unwrap();
    let (low, high) = height
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| {
            (l.min(*v), h.max(*v))
        });
    assert!((0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high));
    assert!(high - low > 0.3, "{low}..{high}");
    assert!(planes.roughness.iter().all(|r| *r > 0.7));
    assert!(planes.metallic.iter().all(|m| *m == 0.0));
}
