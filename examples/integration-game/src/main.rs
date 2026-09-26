//! A game that ships baked ashlar content: one plugin, one component, and the
//! files `integration-content` wrote into `assets/`.
//!
//! ```text
//! cargo run --release -p integration-content   # once, and whenever content changes
//! cargo run --release -p integration-game
//! ```
//!
//! `--screenshot <path>` captures one frame and exits.
use std::path::PathBuf;

use ashlar_bevy::prelude::{
    AshlarBuilding, AshlarBuildingSpawned, AshlarCollider, AshlarFailed, AshlarPlugin, AshlarSpaces,
};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};

/// The game's asset root, where the content step writes.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");

#[derive(Resource)]
struct Capture(Option<PathBuf>);

fn setup(mut commands: Commands, server: Res<AssetServer>) {
    // The whole integration: a building, the library it wears, and a place.
    commands.spawn((
        AshlarBuilding {
            building: server.load("buildings/village.ashlar"),
            materials: server.load("materials/library.materials.ron"),
        },
        Transform::default(),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(20.0, 30.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(18.0, 12.0, 20.0).looking_at(Vec3::new(-2.0, 0.0, -2.0), Vec3::Y),
    ));
}

/// What a game does once a building is in: hand its proxies to physics, its
/// rooms to AI and audio. Here, it says what it got.
fn report(
    mut spawned: MessageReader<AshlarBuildingSpawned>,
    spaces: Query<&AshlarSpaces>,
    colliders: Query<&AshlarCollider>,
) {
    for event in spawned.read() {
        if let Ok(spaces) = spaces.get(event.entity) {
            info!(
                "spawned: {} rooms, {} portals, {} collision proxies",
                spaces.rooms.len(),
                spaces.portals.len(),
                colliders.iter().count()
            );
        }
    }
}

fn fail(failed: Query<&AshlarFailed>, mut exit: MessageWriter<AppExit>) {
    for failure in &failed {
        error!(
            "{}; run `cargo run -p integration-content` first",
            failure.0
        );
        exit.write(AppExit::error());
    }
}

fn capture(
    mut commands: Commands,
    path: Res<Capture>,
    spawned: Query<(), With<ashlar_bevy::prelude::AshlarSpawned>>,
    mut frames: Local<u32>,
) {
    let Some(path) = path.0.clone() else {
        return;
    };
    if spawned.is_empty() {
        return;
    }
    *frames += 1;
    if *frames == 90 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path))
            .observe(
                |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                    exit.write(AppExit::Success);
                },
            );
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut screenshot = None;
    while let Some(argument) = arguments.next() {
        if argument == "--screenshot" {
            screenshot = arguments.next().map(PathBuf::from);
        }
    }
    App::new()
        .add_plugins((
            DefaultPlugins.set(AssetPlugin {
                file_path: ASSETS.to_owned(),
                ..default()
            }),
            AshlarPlugin::default(),
        ))
        .insert_resource(Capture(screenshot))
        .add_systems(Startup, setup)
        .add_systems(Update, (report, fail, capture))
        .run();
}
