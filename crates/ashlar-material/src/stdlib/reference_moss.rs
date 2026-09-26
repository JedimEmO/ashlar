//! Variants share their original substrate graph and exported habitat masks.
use crate::{
    Input, MaterialGraph, MaterialGraphBuilder,
    MathOp::{Add, Max, Mul},
    Param, ParamValue, PbrOutput,
    nodes::{Clamp, Invert, Math, OcclusionFromHeight, Subgraph, SurfaceOutput},
};

/// Layer shared moss over the reference brick, with light or heavy coverage.
pub fn brick_moss(heavy: bool) -> MaterialGraph {
    let name = if heavy {
        "brick-moss-heavy"
    } else {
        "brick-moss-light"
    };
    let g = MaterialGraph::builder(format!("library:{name}"))
        .param(Param::float("moss_amount", if heavy { 0.65 } else { 0.38 }).range(0.0, 1.0))
        .layer(
            "brick",
            Subgraph::new("library:brick"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("mortar_mask".into()),
                SurfaceOutput::Extra("joint_depth".into()),
                SurfaceOutput::Extra("hole_mask".into()),
                SurfaceOutput::Extra("brick_id".into()),
            ],
        )
        .node(
            "habitat",
            Clamp::new(Math::new(
                Max,
                "brick.hole_mask",
                Math::new(
                    Add,
                    Math::new(Mul, "brick.mortar_mask", 0.7),
                    Math::new(Mul, "brick.joint_depth", 0.3),
                ),
            )),
        );
    coat(g, "brick", 0.02, 0.065, "brick_id")
}

/// Layer shared moss over the cobblestone, preserving its 25 mm height scale.
pub fn cobblestone_moss(heavy: bool) -> MaterialGraph {
    let name = if heavy {
        "soi-cobblestone-moss-heavy"
    } else {
        "soi-cobblestone-moss-light"
    };
    let g = MaterialGraph::builder(format!("library:{name}"))
        .param(Param::float("moss_amount", if heavy { 0.65 } else { 0.38 }).range(0.0, 1.0))
        .layer(
            "stone",
            Subgraph::new("library:soi-cobblestone"),
            &[
                SurfaceOutput::BaseColor,
                SurfaceOutput::Roughness,
                SurfaceOutput::Metallic,
                SurfaceOutput::Height,
                SurfaceOutput::Extra("own".into()),
                SurfaceOutput::Extra("dirt".into()),
                SurfaceOutput::Extra("stone_id".into()),
            ],
        )
        .node(
            "habitat",
            Clamp::new(Math::new(
                Add,
                Math::new(Mul, Invert::new("stone.own"), 0.85),
                Math::new(Mul, "stone.dirt", 0.15),
            )),
        );
    coat(g, "stone", 0.0125, 0.104, "stone_id")
}

fn coat(
    g: MaterialGraphBuilder,
    base: &str,
    relief: f32,
    depth: f32,
    identity: &str,
) -> MaterialGraph {
    g.layer(
        "moss",
        Subgraph::new("weathering:moss")
            .input("base_color", format!("{base}.base_color"))
            .input("roughness", format!("{base}.roughness"))
            .input("metallic", format!("{base}.metallic"))
            .input("height", format!("{base}.height"))
            .input("shelter", "habitat")
            .input("coverage", Input::param("moss_amount"))
            .param("amount", ParamValue::Float(1.0))
            .param("depth", ParamValue::Float(depth)),
        &[
            SurfaceOutput::BaseColor,
            SurfaceOutput::Roughness,
            SurfaceOutput::Metallic,
            SurfaceOutput::Height,
            SurfaceOutput::Extra("mask".into()),
        ],
    )
    .node(
        "ao",
        OcclusionFromHeight::new(Math::new(Mul, "moss.height", relief)).radius(0.015),
    )
    .output(
        PbrOutput::new()
            .base_color("moss.base_color")
            .roughness("moss.roughness")
            .metallic("moss.metallic")
            .height("moss.height")
            .occlusion("ao")
            .normal_strength(relief)
            .extra("moss_mask", "moss.mask")
            .extra("habitat", "habitat")
            .extra(identity, format!("{base}.{identity}")),
    )
    // The substrate this coats declares the same repeat, and a coat that
    // declared another would be moss growing at a different scale from the
    // stone under it.
    .tile_metres([super::reference_support::TILE_METRES; 2])
    .into_graph()
}
