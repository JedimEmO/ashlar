//! A game growing baked grass: one cargo feature, one file, no graph engine.
//!
//! `examples/building.rs` is the featureless contract — a recipe, two shipped
//! texture sets, drawn. This is the same contract one step along, for the one
//! thing a game could not ship before 2026-09-20: geometry grown out of a
//! material graph. A strand layer used to be scattered from its graph at
//! runtime, so a lawn meant linking the whole graph engine. It has a baked form
//! now, and this is what reading one costs.
//!
//! The whole of it is [`grow`], and it is about thirty lines: read the file,
//! hand it to [`create_strands`](ashlar_bevy::strands::create_strands) with the
//! triangles the lawn stands on, upload the chunks. The triangles are made here
//! by hand rather than meshed from a building, because that is the point — a
//! game has ground from wherever it has ground, and none of `ashlar`'s geometry
//! half is involved.
//!
//! Export the lawn first — `just materials`, the content step, which writes the
//! default library's files under `assets/materials/` — then build it with
//! `cargo run --example lawn -p ashlar-bevy --features strands`.
//! That one feature pulls `ashlar-strands`, which is `glam`, `serde` and
//! `thiserror`; `cargo tree -e normal --features strands` has no
//! `ashlar-material` in it at all.
//!
//! `--screenshot <path>` captures one frame and exits, which is how CI looks at
//! it without a person in front of it.
use std::path::PathBuf;

use ashlar::{FaceSource, MaterialDefinition, StrandSettings, TriangleMesh, glam::DVec3};
use ashlar_bevy::{
    cards::CardCache,
    strands::{StrandContext, StrandSets, create_strands, strand_material},
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

/// Where the exported assets live, as an absolute path.
///
/// The workspace's `assets`, not this crate's, for `examples/building.rs`'s
/// reason: `ashlar-bevy` ships no content, and the lawn this example grows is
/// the default library's, exported by `just materials`.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

/// The baked lawn, as an asset key under [`ASSETS`].
///
/// Written by `just materials`, beside the KTX2 maps of the same material.
/// Every layer `library:grass` grows is in this one file.
const LAWN: &str = "materials/library/grass/set.strands";

/// Metres to a side of the patch of ground the lawn covers.
const GROUND: f64 = 6.0;

/// The definition the lawn is grown from.
///
/// Note what is *not* in it. There is no material graph named, no `baked_from`
/// to recover one through and no texture set: the surface is `Plain`, and what
/// the definition says about strands is which layers, how many of them, how far
/// out each level of detail reaches, and the file they are in.
///
/// `tile_metres` is the repeat `library:grass` was drawn for. A set's roots
/// are in UV over one repeat, so this is what turns them into metres of ground
/// — and it is why a lawn laid at another tiling warns: a blade's length is
/// absolute metres and does not scale with the repeat under it.
fn lawn() -> MaterialDefinition {
    MaterialDefinition {
        tile_metres: [2.0, 2.0],
        strands: Some(
            StrandSettings::new(["blades", "fibres", "stragglers"])
                // Where each level of detail gives way to the next, and where
                // the last of them gives way to cards. The showcase's own
                // numbers, because they are a property of how big a blade is.
                .lod_metres([2.5, 7.0])
                .card_metres(14.0)
                // A quarter of the authored lawn, which is what a scene that
                // cannot afford sixteen thousand blades a square metre says.
                // It keeps a rank prefix, so lowering it removes blades and
                // never moves the ones that stay.
                .density(0.25)
                // A blade is a tenth of a shadow texel at any cascade that
                // covers a scene, so what a lawn contributes to a shadow map is
                // mostly aliasing.
                .cast_shadows(false)
                .baked_set(LAWN),
        ),
        ..default()
    }
}

/// The ground, as the triangles a lawn is planted on.
///
/// One square of it, with its UVs in **metres** — which is what `ashlar`
/// measures them in, and what `create_strands` divides by the definition's own
/// `tile_metres` to get repeats. A real game's ground comes off a heightfield
/// or a streamer; the only thing placement asks of it is positions, normals,
/// UVs and indices.
fn ground() -> TriangleMesh {
    let half = GROUND * 0.5;
    let corner = |x: f64, z: f64| DVec3::new(x, 0.0, z);
    let mut surface = TriangleMesh::default();
    for (a, b, c) in [
        ((0.0, 0.0), (GROUND, 0.0), (GROUND, GROUND)),
        ((0.0, 0.0), (GROUND, GROUND), (0.0, GROUND)),
    ] {
        surface.push_triangle(
            [
                corner(a.0 - half, a.1 - half),
                corner(b.0 - half, b.1 - half),
                corner(c.0 - half, c.1 - half),
            ],
            [DVec3::Y; 3],
            [[a.0, a.1], [b.0, b.1], [c.0, c.1]],
            FaceSource::BODY,
        );
    }
    surface
}

/// Read the baked lawn and grow it on the ground: the whole of what this
/// example is for.
///
/// Three calls. [`StrandSets::read`] turns the bytes into the sets the
/// definition names; [`create_strands`] plants them on the triangles and hands
/// back one mesh per level of detail per repeat of ground; the loop uploads
/// those and gives each its [`VisibilityRange`], which is what makes a level of
/// detail happen at all.
///
/// A chunk's mesh is written relative to the chunk's own origin, so the origin
/// goes in the transform — and every level of one chunk shares it, which is
/// what makes Bevy's dithered crossfade between two levels exact.
fn grow(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Result {
    let definition = lawn();
    let bytes = std::fs::read(std::path::Path::new(ASSETS).join(LAWN))?;
    let sets = StrandSets::read(LAWN, &bytes)?;
    let mut cards = CardCache::new();
    let mut cx = StrandContext::new(images, &mut cards);
    for layer in create_strands(&definition, &ground(), &sets, &mut cx)? {
        let material = materials.add(strand_material(&definition, layer.roughness));
        info!(
            "{}: {} strands, {} triangles over {} levels",
            layer.layer,
            layer.strands,
            layer.triangles,
            layer.levels.len()
        );
        for level in &layer.levels {
            for chunk in &level.chunks {
                let mut entity = commands.spawn((
                    Mesh3d(meshes.add(chunk.mesh.clone())),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(Vec3::from_array(chunk.origin)),
                ));
                if let Some(range) = &level.level.range {
                    entity.insert(range.clone());
                }
            }
        }
    }
    Ok(())
}

/// Where a captured frame goes, or nothing for a run with somebody watching.
#[derive(Resource)]
struct Capture(Option<PathBuf>);

/// The lawn, a patch of earth under it, a key light and a camera low enough to
/// see the blades against the sky.
fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) -> Result {
    grow(&mut commands, &mut meshes, &mut materials, &mut images)?;
    // The earth under the lawn. A real scene's ground wears the same material's
    // baked maps; this one is a colour, so that what the picture shows is the
    // geometry rather than the texture.
    // The renderer is the f64-to-f32 boundary, here as everywhere in this
    // stack: `ashlar` authors metres in `f64` and Bevy draws them in `f32`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "an authored extent, written as a literal one decimal long"
    )]
    let side = GROUND as f32;
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(side, 0.1, side))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::linear_rgb(0.035, 0.030, 0.018),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));
    // Half a bright day. A lawn is a canopy and the thing worth seeing in it is
    // the dark under the blades; at full daylight a bare patch of earth washes
    // out and takes that contrast with it.
    commands.spawn((
        DirectionalLight {
            illuminance: 5_500.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 6.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Low and close: a lawn seen from standing height is a texture, and the
    // whole of what the geometry buys is the silhouette at the near edge.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 0.22, 1.15).looking_at(Vec3::new(0.0, 0.06, -0.9), Vec3::Y),
    ));
    Ok(())
}

/// Capture one frame and leave, for a run with nobody in front of it.
fn capture(
    mut commands: Commands,
    capture: Res<Capture>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    *frames += 1;
    let Some(path) = capture.0.clone() else {
        return;
    };
    // Fewer frames than `building.rs` waits: nothing here loads from disk
    // through the asset server, so what is being waited on is the first
    // pipeline rather than several megabytes of KTX2.
    if *frames != 60 {
        if *frames > 600 {
            exit.write(AppExit::Success);
        }
        return;
    }
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            },
        );
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let screenshot = arguments
        .find(|argument| argument == "--screenshot")
        .and(arguments.next())
        .map(PathBuf::from);
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: ASSETS.to_owned(),
            ..default()
        }))
        .insert_resource(Capture(screenshot))
        .insert_resource(ClearColor(Color::srgb(0.42, 0.55, 0.68)))
        .add_systems(Startup, spawn)
        .add_systems(Update, capture)
        .run();
}
